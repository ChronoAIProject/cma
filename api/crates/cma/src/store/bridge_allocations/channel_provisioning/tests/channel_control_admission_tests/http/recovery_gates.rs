use super::*;
use crate::bridge::ingress::reply_addressing::{ReplyAddressingStore, ReplyLookup};
use layer2_core::bridge::partition::{ChannelAdmissionPolicy, GroupAdmission, IdentityAdmission};

async fn fail_reply_lookup(c: &ControlFixture, value: &Value) {
    let lookup = ReplyLookup::retained(&legacy_source(c, value).await).unwrap();
    c.f.client
        .database(&c.f.database)
        .collection::<Document>("bridge_outbox_items")
        .update_one(doc! {"_id":&c.replied.id}, doc! {"$set":{"revision":0_i64}})
        .await
        .unwrap();
    assert!(c.f.replies.confirmed_reply(&lookup).await.is_err());
}

async fn require_current_reply(c: &ControlFixture) {
    let id = &c.f.inbox.pin.bridge_id;
    let bridge = c.f.bridges.bridge(id).await.unwrap().unwrap();
    c.f.svc
        .mutate(
            &caller(),
            None,
            change(
                id,
                bridge.revision,
                "recovery-policy-disable",
                BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::DrainOriginal,
                },
            ),
        )
        .await
        .unwrap();
    let bridge = c.f.bridges.bridge(id).await.unwrap().unwrap();
    let mut config = bridge.current().unwrap().config.clone();
    config.admission = BridgeAdmissionPolicy::Scoped {
        policy: ChannelAdmissionPolicy {
            senders: IdentityAdmission::Open,
            chats: IdentityAdmission::Open,
            groups: GroupAdmission::ReplyToBot,
        },
    };
    c.f.svc
        .mutate(
            &caller(),
            None,
            change(
                id,
                bridge.revision,
                "recovery-policy-update",
                BridgeChange::Update {
                    config: Box::new(config),
                    previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
                },
            ),
        )
        .await
        .unwrap();
    let bridge = c.f.bridges.bridge(id).await.unwrap().unwrap();
    c.f.svc
        .mutate(
            &caller(),
            None,
            change(
                id,
                bridge.revision,
                "recovery-policy-enable",
                BridgeChange::Enable,
            ),
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_reply_lookup_failure_recovers_complete_duplicate_and_provider_alias()
{
    let c = ControlFixture::with_callback_route().await;
    let value = body(&c, "lookup-original", 740, false);
    let (status, accepted) = post(app(&c, true, Some(Runtime::local(&c))).await, &c, &value).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    fail_reply_lookup(&c, &value).await;
    let runtime = Runtime::local(&c);
    for (index, value) in [value, body(&c, "lookup-alias", 740, false)]
        .into_iter()
        .enumerate()
    {
        let (status, result) = post(app(&c, false, Some(runtime.clone())).await, &c, &value).await;
        assert_eq!(status, StatusCode::ACCEPTED,
            "retained duplicate and new provider alias must both recover despite current reply lookup failure; case {index}: {result}");
        assert_eq!(result, accepted);
    }
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 0);
    assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 2);
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 1);
    assert_eq!(rows(&c, "agent_commands").await, 2);
    assert_eq!(rows(&c, "bridge_callback_inbox").await, 1);
    assert_eq!(rows(&c, "bridge_outbox_items").await, 1);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_reply_lookup_failure_cannot_acknowledge_incomplete_acceptance() {
    for damage in ["intent", "command", "provider"] {
        let c = ControlFixture::with_callback_route().await;
        let value = body(&c, "lookup-incomplete", 741, false);
        let (status, accepted) =
            post(app(&c, true, Some(Runtime::local(&c))).await, &c, &value).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
        fail_reply_lookup(&c, &value).await;
        let (collection, query) = match damage {
            "intent" => ("bridge_channel_control_intents", doc! {}),
            "command" => (
                "agent_commands",
                doc! {"channel_control_admission":{"$exists":true}},
            ),
            _ => (
                "bridge_callback_dispositions",
                doc! {"kind":"provider","record.version":2_i32},
            ),
        };
        let removed =
            c.f.client
                .database(&c.f.database)
                .collection::<Document>(collection)
                .delete_one(query)
                .await
                .unwrap();
        assert_eq!(removed.deleted_count, 1, "{damage}");
        let dispositions = rows(&c, "bridge_callback_dispositions").await;
        let (status, result) = post(app(&c, false, None).await, &c, &value).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "lookup failure cannot bypass complete retained {damage} evidence: {result}"
        );
        assert_eq!(result["acceptance"], "unknown");
        assert_eq!(rows(&c, "bridge_callback_dispositions").await, dispositions);
        assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 1);
        assert_eq!(rows(&c, "bridge_callback_inbox").await, 1);
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_current_group_refusal_still_recovers_complete_original_alias() {
    let c = ControlFixture::with_callback_route().await;
    let original = body(&c, "group-original", 742, false);
    let (status, accepted) =
        post(app(&c, true, Some(Runtime::local(&c))).await, &c, &original).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    require_current_reply(&c).await;
    c.f.client
        .database(&c.f.database)
        .collection::<Document>("bridge_outbox_items")
        .delete_one(doc! {"_id":&c.replied.id})
        .await
        .unwrap();
    let alias = body(&c, "group-alias", 742, false);
    let (status, result) = post(app(&c, false, None).await, &c, &alias).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{result}");
    assert_eq!(result, accepted);
    assert_eq!(
        rows(&c, "bridge_channel_control_aliases").await,
        2,
        "current group admission cannot bypass the control alias and complete intent transaction"
    );
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 1);
    assert_eq!(rows(&c, "agent_commands").await, 2);
    assert_eq!(rows(&c, "bridge_callback_inbox").await, 1);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_fresh_lookup_failure_never_creates_acceptance_or_refusal() {
    let c = ControlFixture::with_callback_route().await;
    let value = body(&c, "lookup-fresh", 743, false);
    fail_reply_lookup(&c, &value).await;
    let dispositions = rows(&c, "bridge_callback_dispositions").await;
    let runtime = Runtime::local(&c);
    for enabled in [false, true] {
        let (status, result) =
            post(app(&c, enabled, Some(runtime.clone())).await, &c, &value).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{result}");
        assert_eq!(result["acceptance"], "unknown");
        assert_eq!(rows(&c, "bridge_callback_dispositions").await, dispositions);
    }
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 0);
    assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 0);
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 0);
    assert_eq!(rows(&c, "agent_commands").await, 1);
    assert_eq!(rows(&c, "bridge_callback_inbox").await, 1);
    c.f.close().await;
}
