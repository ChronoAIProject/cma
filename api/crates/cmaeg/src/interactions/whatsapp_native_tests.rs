use super::{
    whatsapp_authority::route,
    whatsapp_native_fixture::{now, Fixture},
    Error,
};
use crate::channels::definition::fixture as definition_fixture;
use cmaeg_core::activity::SourceKind;
use serde_json::json;

#[tokio::test]
async fn native_whatsapp_controls_keep_default_and_named_owner_and_exact_replay() {
    for named in [false, true] {
        for list in [false, true] {
            let mut f = Fixture::new(named).await;
            let card = f.card(list, true).await;
            let locator = Fixture::locator(&card, list);
            let activity = f.activity(&locator, "wamid.click");
            let route = route(&activity.source, &activity.actor.id).unwrap();
            let original = f.event.clone();
            let channel = std::sync::Arc::make_mut(&mut f.channel);
            channel
                .definition
                .routing
                .as_mut()
                .unwrap()
                .default_targets
                .clear();
            channel.definition.partition.version += 1;
            let result = f
                .store
                .consume_for_dispatch(
                    &f.channel,
                    &activity,
                    &route,
                    "wamid.click",
                    &locator,
                    "wamid.confirmed",
                    None,
                )
                .await
                .unwrap();
            assert_eq!(
                result.0.action_id,
                if list { "private-read" } else { "private-yes" }
            );
            assert_eq!(
                result.0.values["selected"],
                json!([if list { "private-read" } else { "private-yes" }])
            );
            let owner = result.2.as_ref().unwrap();
            assert_eq!(owner.consumer, original.consumer);
            assert_eq!(owner.conversation_id, original.conversation_id);
            assert_eq!(owner.partition_key, original.partition_key);
            assert_eq!(owner.routing.as_ref().unwrap().targets.len(), 1);
            let mut replay = activity.clone();
            replay.occurred_at = "2026-10-28T00:00:00Z".into();
            let again = f
                .store
                .consume_for_dispatch(
                    &f.channel,
                    &replay,
                    &route,
                    "wamid.click",
                    &locator,
                    "wamid.confirmed",
                    None,
                )
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(&result.0).unwrap(),
                serde_json::to_value(&again.0).unwrap()
            );
            assert_eq!(result.1, again.1);
            assert!(result.2 == again.2);
        }
    }
}

#[tokio::test]
async fn native_whatsapp_controls_refuse_changed_authority_without_consuming() {
    for named in [false, true] {
        for case in 0..17 {
            let mut f = Fixture::new(named).await;
            let card = f.card(false, true).await;
            let mut locator = Fixture::locator(&card, false);
            let mut activity = f.activity(&locator, "wamid.click");
            let original_route = route(&activity.source, &activity.actor.id).unwrap();
            let mut destination = "wamid.confirmed";
            match case {
                0 => activity.actor.id = "447700900999".into(),
                1 => activity.conversation.id = "447700900999".into(),
                2 => {
                    let SourceKind::Whatsapp {
                        phone_number_id, ..
                    } = &mut activity.source
                    else {
                        panic!()
                    };
                    *phone_number_id = "99999".into();
                }
                3 => {
                    let SourceKind::Whatsapp { waba_id, .. } = &mut activity.source else {
                        panic!()
                    };
                    *waba_id = "99999".into();
                }
                4 => destination = "wamid.foreign",
                5 => locator = locator.replace("cw1b:", "cw1l:"),
                6 => locator = locator.replace(":1:", ":2:"),
                7 => std::sync::Arc::make_mut(&mut f.channel)
                    .definition
                    .sources
                    .clear(),
                8 => {
                    std::sync::Arc::make_mut(&mut f.channel)
                        .definition
                        .secrets
                        .whatsapp = None
                }
                9 => {
                    let channel = std::sync::Arc::make_mut(&mut f.channel);
                    if named {
                        channel.definition.routing.as_mut().unwrap().consumers[0]
                            .provider
                            .binding_id = "different-binding".parse().unwrap();
                    } else {
                        definition_fixture::replace_default_binding(
                            &mut channel.definition,
                            "different-binding".parse().unwrap(),
                        );
                    }
                }
                10 => activity.conversation.thread_id = Some("thread".into()),
                11 => activity.actor.kind = cmaeg_core::activity::ActorKind::Human,
                12 => {
                    let cmaeg_core::channel::Source::Whatsapp { admission, .. } =
                        &mut std::sync::Arc::make_mut(&mut f.channel).definition.sources[0]
                    else {
                        panic!()
                    };
                    *admission = cmaeg_core::channel::AdmissionPolicy::Allowlist {
                        sender_ids: vec![activity.actor.id.clone()],
                    };
                }
                13 => {
                    let definition = &mut std::sync::Arc::make_mut(&mut f.channel).definition;
                    if named {
                        definition.secrets.consumer_keys.clear();
                    } else {
                        definition_fixture::replace_default_credential(definition, None);
                    }
                }
                14 if named => {
                    std::sync::Arc::make_mut(&mut f.channel)
                        .definition
                        .routing
                        .as_mut()
                        .unwrap()
                        .consumers[0]
                        .pending = true
                }
                15 if named => std::sync::Arc::make_mut(&mut f.channel)
                    .definition
                    .routing
                    .as_mut()
                    .unwrap()
                    .consumers
                    .clear(),
                _ => {
                    std::sync::Arc::make_mut(&mut f.channel).pointer.record_id =
                        uuid::Uuid::new_v4().to_string().parse().unwrap()
                }
            }
            assert!(
                matches!(
                    f.store
                        .consume_for_dispatch(
                            &f.channel,
                            &activity,
                            &original_route,
                            "wamid.click",
                            &locator,
                            destination,
                            None
                        )
                        .await,
                    Err(Error::Denied)
                ),
                "case {case}, named {named}"
            );
            assert!(f
                .store
                .load(&card.card.id)
                .await
                .unwrap()
                .0
                .consumed
                .is_none());
        }
    }
}

#[tokio::test]
async fn native_whatsapp_controls_allow_valid_secret_rotation_with_original_source_and_provider() {
    for named in [false, true] {
        let mut f = Fixture::new(named).await;
        let card = f.card(false, true).await;
        let locator = Fixture::locator(&card, false);
        let activity = f.activity(&locator, "wamid.rotated");
        let route = route(&activity.source, &activity.actor.id).unwrap();
        let secrets = &mut std::sync::Arc::make_mut(&mut f.channel).definition.secrets;
        let native = secrets.whatsapp.as_mut().unwrap();
        native.app_secret = cmaeg_core::channel::Secret::new("rotated-fixture-app-secret");
        native.access_token = cmaeg_core::channel::Secret::new("rotated-fixture-access-token");
        native.verify_token = cmaeg_core::channel::Secret::new("rotated-fixture-verify-token");
        if named {
            let key = secrets
                .consumer_keys
                .iter_mut()
                .find(|(id, _)| Some(id) == f.event.consumer.as_ref())
                .unwrap();
            key.1 = cmaeg_core::channel::Secret::new("rotated-fixture-provider-credential");
        } else {
            definition_fixture::replace_default_agent_key(
                &mut std::sync::Arc::make_mut(&mut f.channel).definition,
                "rotated-fixture-provider-credential",
            );
        }
        assert!(f
            .store
            .consume_for_dispatch(
                &f.channel,
                &activity,
                &route,
                "wamid.rotated",
                &locator,
                "wamid.confirmed",
                None
            )
            .await
            .is_ok());
    }
}

#[tokio::test]
async fn native_whatsapp_pending_expired_superseded_and_concurrent_controls_are_fenced() {
    let f = Fixture::new(false).await;
    let mut card = f.card(false, false).await;
    let locator = Fixture::locator(&card, false);
    let activity = f.activity(&locator, "wamid.click");
    let route = route(&activity.source, &activity.actor.id).unwrap();
    assert!(matches!(
        f.store
            .consume_for_dispatch(
                &f.channel,
                &activity,
                &route,
                "wamid.click",
                &locator,
                "wamid.confirmed",
                None
            )
            .await,
        Err(Error::Unavailable)
    ));
    f.store
        .bind(
            &mut card,
            super::Destination {
                outbound_id: "wamid.confirmed".into(),
                platform_id: "wamid.confirmed".into(),
            },
        )
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        f.store.consume_for_dispatch(
            &f.channel,
            &activity,
            &route,
            "wamid.click",
            &locator,
            "wamid.confirmed",
            None
        ),
        f.store.consume_for_dispatch(
            &f.channel,
            &activity,
            &route,
            "wamid.click",
            &locator,
            "wamid.confirmed",
            None
        )
    );
    assert_eq!(
        serde_json::to_value(a.unwrap().0).unwrap(),
        serde_json::to_value(b.unwrap().0).unwrap()
    );
    let other = f.activity(&locator, "wamid.other");
    assert!(matches!(
        f.store
            .consume_for_dispatch(
                &f.channel,
                &other,
                &route,
                "wamid.other",
                &locator,
                "wamid.confirmed",
                None
            )
            .await,
        Err(Error::Denied)
    ));
    for expiry in [false, true] {
        let fresh = f.card(false, true).await;
        let (mut saved, version) = f.store.load(&fresh.card.id).await.unwrap();
        if expiry {
            f.server.change(&fresh.card.id, |value| {
                value["expires_at"] = json!(now() - 1)
            });
        } else {
            saved.superseded = true;
            f.store.save(&saved, Some(version)).await.unwrap();
        }
        let loc = Fixture::locator(&fresh, false);
        assert!(f
            .store
            .consume_for_dispatch(
                &f.channel,
                &activity,
                &route,
                "wamid.click",
                &loc,
                "wamid.confirmed",
                None
            )
            .await
            .is_err());
    }
}
