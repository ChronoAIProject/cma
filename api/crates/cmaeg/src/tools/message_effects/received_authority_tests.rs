use super::{received_fixture::*, *};
use crate::channels::definition::fixture as definition_fixture;
use cmaeg_core::{
    activity::{ActivityKind, ActorKind, SourceKind},
    channel::{Secret, Source},
    seal::event_ref::activity_digest,
};
use serde_json::json;

#[tokio::test]
async fn received_reaction_current_custody_revocation_precedes_provider_writes() {
    for native in [Native::Telegram, Native::Discord] {
        for defect in 0..9 {
            let mut f = Fixture::new(native, false, false).await;
            let reference = f.reference().await;
            match defect {
                0 => f.app.definition.sources.clear(),
                1 => match native {
                    Native::Telegram => {
                        f.app.definition.secrets.telegram_bot_token =
                            Some(Secret::new("77:replacement-token-fixture"))
                    }
                    Native::Discord => {
                        f.app.definition.secrets.discord_bot_token =
                            Some(Secret::new("replacement-discord-fixture"))
                    }
                },
                2 => f.app.definition.owner.subject = "different-owner".into(),
                3 => definition_fixture::replace_default_credential(
                    &mut f.app.definition,
                    Some("new-provider-credential"),
                ),
                4 => definition_fixture::replace_default_binding(
                    &mut f.app.definition,
                    "replacement-provider".parse().unwrap(),
                ),
                5 => match &mut f.app.definition.sources[0] {
                    Source::Telegram { admission, .. } | Source::Discord { admission, .. } => {
                        *admission =
                            serde_json::from_value(json!({"type":"allowlist","sender_ids":["42"]}))
                                .unwrap()
                    }
                    _ => unreachable!(),
                },
                6 => f
                    .app
                    .definition
                    .routing
                    .as_mut()
                    .unwrap()
                    .default_targets
                    .clear(),
                7 => {
                    if native == Native::Telegram {
                        f.app.definition.secrets.telegram_webhook_secret =
                            Some(Secret::new("rotated-webhook-fixture"));
                    } else {
                        f.app.definition.secrets.discord_bot_token = None;
                    }
                }
                8 => definition_fixture::replace_default_agent_key(
                    &mut f.app.definition,
                    "nyxid_ag_rotated-received-fixture",
                ),
                _ => unreachable!(),
            }
            f.app.replace();
            assert!(
                f.app
                    .state
                    .set_message_reaction(f.app.caller(), &reference, "revoked", request("👍"))
                    .await
                    .is_err(),
                "{native:?}: defect {defect}"
            );
            assert_eq!(f.writes(), 0);
        }
    }
}

#[tokio::test]
async fn received_reaction_sealed_exact_event_target_actor_and_consumer_cannot_be_retargeted() {
    for native in [Native::Telegram, Native::Discord] {
        let f = Fixture::new(native, true, true).await;
        let reference = f.reference().await;
        let channel = f
            .app
            .state
            .resolver
            .resolve_current(f.app.event.channel_id.as_str())
            .await
            .unwrap();
        let default_caller = AgentCaller {
            key_hash: *channel.consumer_key_hash(None).unwrap(),
        };
        assert!(f
            .app
            .state
            .set_message_reaction(default_caller, &reference, "foreign", request("👍"))
            .await
            .is_err());
        for defect in 0..10 {
            let mut payload = reference.open(&f.app.state.keys).unwrap().payload;
            let a = payload.received_reaction.as_mut().unwrap();
            match defect {
                0 => payload.platform_message_id = "801".into(),
                1 => payload.event_id = "forged-event".parse().unwrap(),
                2 => a.actor_id = "43".into(),
                3 => a.chat_id = "100".into(),
                4 => a.bot_id = "78".into(),
                5 => a.thread_id = None,
                6 => a.event_sha256 = "f".repeat(64),
                7 => payload.consumer = None,
                8 => a.received_at -= 1,
                9 => a.expires_at += 1,
                _ => unreachable!(),
            }
            payload.route_id = a.route();
            let forged = MessageRef::seal(&payload, &f.app.state.keys).unwrap();
            assert!(
                f.app
                    .state
                    .set_message_reaction(f.app.caller(), &forged, "forged", request("👍"))
                    .await
                    .is_err(),
                "{native:?}: defect {defect}"
            );
        }
        assert_eq!(f.writes(), 0);
    }
}

#[tokio::test]
async fn received_reaction_only_ordinary_exact_human_messages_with_original_proof() {
    for native in [Native::Telegram, Native::Discord] {
        for defect in 0..7 {
            let mut f = Fixture::new(native, false, true).await;
            match defect {
                0 => f.app.activity.actor.kind = ActorKind::Unknown,
                1 => f.app.activity.actor.kind = ActorKind::Bot,
                2 => f.app.activity.actor.kind = ActorKind::System,
                3 => {
                    if let ActivityKind::Message { album_id, .. } = &mut f.app.activity.kind {
                        *album_id = Some("album".into())
                    }
                }
                4 => f.app.activity.source = SourceKind::Custom,
                5 => f.app.activity.conversation.thread_id = Some("124".into()),
                6 => f.app.activity.conversation.id = "100".into(),
                _ => unreachable!(),
            }
            f.app.event.activity_sha256 = Some(activity_digest(&f.app.activity));
            let channel = f
                .app
                .state
                .resolver
                .resolve_current(f.app.event.channel_id.as_str())
                .await
                .unwrap();
            assert!(
                received_authority::issue(
                    &channel,
                    &f.app.event,
                    &f.app.activity,
                    &f.app.state.keys
                )
                .is_none(),
                "{native:?}: defect {defect}"
            );
            assert!(f
                .app
                .state
                .received_message_context(f.app.caller(), &f.app.event, &f.app.activity, true)
                .await
                .is_none());
            assert_eq!(f.writes(), 0);
        }
    }
    let mut f = Fixture::new(Native::Telegram, false, false).await;
    f.app.event.telegram_admission = None;
    let channel = f
        .app
        .state
        .resolver
        .resolve_current(f.app.event.channel_id.as_str())
        .await
        .unwrap();
    assert!(
        received_authority::issue(&channel, &f.app.event, &f.app.activity, &f.app.state.keys)
            .is_none()
    );
}

#[tokio::test]
async fn received_reaction_fixed_source_age_and_event_expiry_never_extend_on_remint() {
    for native in [Native::Telegram, Native::Discord] {
        let mut f = Fixture::new(native, false, false).await;
        let channel = f
            .app
            .state
            .resolver
            .resolve_current(f.app.event.channel_id.as_str())
            .await
            .unwrap();
        let current = now();
        for (age, expiry, expected) in [
            (86401, current + 600, false),
            (86400, current + 600, false),
            (86280, current + 600, true),
            (60, current, false),
            (-60, current + 600, false),
        ] {
            f.app.activity.occurred_at = time::OffsetDateTime::from_unix_timestamp(current - age)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap();
            f.app.event.activity_sha256 = Some(activity_digest(&f.app.activity));
            f.app.event.expires_at = expiry;
            let issued = received_authority::issue(
                &channel,
                &f.app.event,
                &f.app.activity,
                &f.app.state.keys,
            );
            assert_eq!(issued.is_some(), expected, "{native:?}: age {age}");
            if let Some(issued) = issued {
                assert_eq!(
                    issued
                        .open(&f.app.state.keys)
                        .unwrap()
                        .payload
                        .received_reaction
                        .unwrap()
                        .expires_at,
                    current + 120
                );
            }
        }
    }
}

#[tokio::test]
async fn received_reaction_original_source_and_provider_binding_required_for_remint() {
    for native in [Native::Telegram, Native::Discord] {
        for defect in 0..3 {
            let mut f = Fixture::new(native, false, false).await;
            f.reference().await;
            match defect {
                0 => match &mut f.app.definition.sources[0] {
                    Source::Telegram { admission, .. } | Source::Discord { admission, .. } => {
                        *admission =
                            serde_json::from_value(json!({"type":"allowlist","sender_ids":["42"]}))
                                .unwrap()
                    }
                    _ => unreachable!(),
                },
                1 => definition_fixture::replace_default_binding(
                    &mut f.app.definition,
                    "replacement-provider".parse().unwrap(),
                ),
                2 => definition_fixture::replace_default_credential(&mut f.app.definition, None),
                _ => unreachable!(),
            }
            f.app.replace();
            f.app.restart();
            assert!(
                f.context().await.received_message.is_none(),
                "{native:?}: defect {defect}"
            );
            assert_eq!(f.writes(), 0);
        }
    }
}
