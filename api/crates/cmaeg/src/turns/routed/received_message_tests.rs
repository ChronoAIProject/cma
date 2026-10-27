use super::*;
use crate::channels::definition::fixture as definition_fixture;
use crate::ingress::{
    launch::LaunchRequest, routing_plan::fixture, routing_store::test_server::Server,
};
use cmaeg_core::{
    activity::{ActorKind, SourceKind},
    channel::{AdmissionPolicy, Source},
    seal::{event_ref::activity_digest, EventRef, ReplyAddress},
};
use serde_json::json;
use std::sync::{atomic::AtomicUsize, Mutex};

fn request() -> LaunchRequest {
    let mut activity = fixture::message("received native message");
    activity.source = SourceKind::Whatsapp {
        app_id: "12345".into(),
        waba_id: "23456".into(),
        phone_number_id: "34567".into(),
    };
    activity.actor.id = "447700900123".into();
    activity.actor.kind = ActorKind::Unknown;
    activity.conversation.id = activity.actor.id.clone();
    let mut request = fixture::request(
        |value| {
            value["sources"] = json!([crate::whatsapp::tests::source()]);
            let credentials = crate::whatsapp::tests::credentials();
            value["secrets"]["whatsapp"] = json!({
                "app_secret":credentials.app_secret.as_str(),
                "access_token":credentials.access_token.as_str(),
                "verify_token":credentials.verify_token.as_str(),
            });
            value["routing"]["default_targets"] = json!(["default"]);
        },
        activity,
    );
    fixture::rewrite_ref(&mut request, |event| {
        event.expires_at = time::OffsetDateTime::now_utc().unix_timestamp() + 600;
        event.reply = ReplyAddress::Whatsapp {
            app_id: "12345".into(),
            waba_id: "23456".into(),
            phone_number_id: "34567".into(),
            recipient_id: "447700900123".into(),
            message_id: "wamid.original".into(),
        };
    });
    request
}

fn authority(channel: Arc<ResolvedChannel>) -> super::super::test_support::Current {
    super::super::test_support::Current {
        channel: Mutex::new(channel),
        calls: AtomicUsize::new(0),
        revoke_at: AtomicUsize::new(usize::MAX),
    }
}

#[tokio::test]
async fn received_message_requires_exact_durable_event_activity_and_original_binding() {
    let server = Server::start_realtime().await;
    let request = request();
    let current = authority(request.channel.clone());
    let keys = Arc::new(fixture::keys());
    let caller = AgentCaller { key_hash: [7; 32] };
    let original = request
        .event_ref
        .open(&keys, time::OffsetDateTime::now_utc().unix_timestamp())
        .unwrap()
        .payload;
    assert!(current_received_message(
        server.records.clone(),
        keys.clone(),
        &current,
        caller,
        &original,
        &request.activity,
    )
    .await
    .is_err());
    let (activity, event) =
        bind_received_message_fixture(server.records.clone(), keys.clone(), &request, None).await;
    assert!(current_received_message(
        server.records.clone(),
        keys.clone(),
        &current,
        caller,
        &event,
        &activity,
    )
    .await
    .is_ok());
    for defect in 0..3 {
        let mut changed_event = event.clone();
        let mut changed_activity = activity.clone();
        match defect {
            0 => changed_event.partition_key.push_str(":different"),
            1 => changed_event.consumer = Some(fixture::id(fixture::REVIEWER)),
            2 => {
                changed_activity.actor.id = "447700900999".into();
                changed_event.activity_sha256 = Some(activity_digest(&changed_activity));
            }
            _ => unreachable!(),
        }
        // Even a well-formed resealed reference cannot replace the original binding.
        EventRef::seal(&changed_event, &keys).unwrap();
        assert!(
            current_received_message(
                server.records.clone(),
                keys.clone(),
                &current,
                caller,
                &changed_event,
                &changed_activity,
            )
            .await
            .is_err(),
            "defect {defect}"
        );
    }
}

#[tokio::test]
async fn received_message_remint_refuses_changed_source_provider_or_missing_current_credential() {
    let server = Server::start_realtime().await;
    let request = request();
    let keys = Arc::new(fixture::keys());
    let (activity, event) =
        bind_received_message_fixture(server.records.clone(), keys.clone(), &request, None).await;
    for defect in 0..6 {
        let mut channel = (*request.channel).clone();
        match defect {
            0 => {
                let Source::Whatsapp { admission, .. } = &mut channel.definition.sources[0] else {
                    panic!()
                };
                *admission = AdmissionPolicy::Allowlist {
                    sender_ids: vec!["447700900999".into()],
                };
            }
            1 => channel.definition.sources.clear(),
            2 => definition_fixture::replace_default_binding(
                &mut channel.definition,
                "different-binding".parse().unwrap(),
            ),
            3 => definition_fixture::replace_default_credential(&mut channel.definition, None),
            4 => channel.definition.owner.subject = "different-owner".into(),
            5 => channel.agent_key_hash = [9; 32],
            _ => unreachable!(),
        }
        assert!(
            matches!(
                current_received_message(
                    server.records.clone(),
                    keys.clone(),
                    &authority(Arc::new(channel)),
                    AgentCaller { key_hash: [7; 32] },
                    &event,
                    &activity,
                )
                .await,
                Err(ExecutionError::AuthorityChanged)
            ),
            "defect {defect}"
        );
    }
    // Routed execution allows a currently authorized replacement credential.
    // The caller issuing a reaction reference then hashes this current revision.
    let mut rotated = (*request.channel).clone();
    definition_fixture::replace_default_credential(
        &mut rotated.definition,
        Some("replacement-consumer-credential"),
    );
    assert!(current_received_message(
        server.records.clone(),
        keys,
        &authority(Arc::new(rotated)),
        AgentCaller { key_hash: [7; 32] },
        &event,
        &activity,
    )
    .await
    .is_ok());
}

#[path = "native_lark_media_authority_tests.rs"]
mod native_lark_media;
