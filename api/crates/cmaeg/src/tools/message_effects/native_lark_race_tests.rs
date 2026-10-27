use super::{native_lark_fixture::Fixture, *};
use cmaeg_core::channel::{LarkRegion, Secret, Source};
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn native_lark_source_owner_consumer_and_provider_changes_revoke_before_io() {
    for named in [false, true] {
        for change in [
            "app",
            "tenant",
            "bot",
            "region",
            "source",
            "secret",
            "encrypt",
            "verification",
            "owner",
            "provider",
            "agent",
            "routing",
        ] {
            let mut f = Fixture::new(LarkRegion::Feishu, named, false).await;
            let reference = f.received().await;
            match change {
                "secret" => {
                    f.app
                        .definition
                        .secrets
                        .native_lark
                        .as_mut()
                        .unwrap()
                        .app_secret = Secret::new("rotated-native-app-secret")
                }
                "encrypt" => {
                    f.app
                        .definition
                        .secrets
                        .native_lark
                        .as_mut()
                        .unwrap()
                        .encrypt_key = Secret::new("rotated-native-encrypt-key")
                }
                "verification" => {
                    f.app
                        .definition
                        .secrets
                        .native_lark
                        .as_mut()
                        .unwrap()
                        .verification_token = Secret::new("rotated-native-verification")
                }
                "owner" => f.app.definition.owner.subject = "other-owner".into(),
                "provider" if named => {
                    f.app.definition.routing.as_mut().unwrap().consumers[0]
                        .provider
                        .binding_id = "other-binding".parse().unwrap()
                }
                "provider" => crate::channels::definition::fixture::replace_default_binding(
                    &mut f.app.definition,
                    "other-binding".parse().unwrap(),
                ),
                "agent" if named => {
                    f.app.definition.routing.as_mut().unwrap().consumers[0].agent_key_hash =
                        format!("sha256:{}", "0".repeat(64));
                }
                "agent" => {
                    f.app.definition.secrets.agent_key_hash = format!("sha256:{}", "0".repeat(64))
                }
                "routing" => f
                    .app
                    .definition
                    .routing
                    .as_mut()
                    .unwrap()
                    .default_targets
                    .clear(),
                "source" => f.app.definition.sources.clear(),
                _ => {
                    let Source::NativeLark(source) = &mut f.app.definition.sources[0] else {
                        panic!()
                    };
                    match change {
                        "app" => source.identity.app_id = "cli_other".into(),
                        "tenant" => source.identity.tenant_key = "other".into(),
                        "bot" => source.identity.bot_open_id = "ou_other".into(),
                        _ => source.identity.region = LarkRegion::Lark,
                    }
                }
            }
            f.app.replace();
            assert!(
                f.react(&reference, "react", Some("SMILE")).await.is_err(),
                "{named} {change}"
            );
            assert!(f.api.calls.lock().unwrap().is_empty(), "{named} {change}");
        }
    }
}
#[tokio::test]
async fn native_lark_metadata_read_races_recheck_local_authority_before_write() {
    for named in [false, true] {
        let f = Fixture::new(LarkRegion::Feishu, named, false).await;
        let reference = f.received().await;
        f.change_at("GET /open-apis/im/v1/messages/om_received", |d| {
            d.secrets.native_lark.as_mut().unwrap().verification_token =
                Secret::new("rotated-native-verification");
        });
        assert_eq!(
            f.react(&reference, "react", Some("SMILE"))
                .await
                .unwrap()
                .delivery
                .kind(),
            "rejected"
        );
        assert!(f.writes().is_empty());
    }
}
#[tokio::test]
async fn native_lark_reaction_replacement_revocation_after_delete_stays_fenced() {
    let mut f = Fixture::new(LarkRegion::Feishu, true, true).await;
    let reference = f.received().await;
    assert_eq!(
        f.react(&reference, "add", Some("SMILE"))
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    f.change_at(
        "DELETE /open-apis/im/v1/messages/om_received/reactions/own_reaction",
        |d| {
            d.secrets.native_lark.as_mut().unwrap().app_secret =
                Secret::new("rotated-native-app-secret");
        },
    );
    assert_eq!(
        f.react(&reference, "replace", Some("THUMBSUP"))
            .await
            .unwrap()
            .delivery
            .kind(),
        "unknown"
    );
    assert_eq!(f.writes().len(), 2);
    f.app.replace();
    f.app.restart();
    assert_eq!(
        f.react(&reference, "new-key", Some("THUMBSUP"))
            .await
            .unwrap()
            .delivery
            .kind(),
        "unknown"
    );
    assert_eq!(f.writes().len(), 2);
}
#[tokio::test]
async fn native_lark_uncertain_and_lost_storage_fence_every_mutation_after_restart() {
    for failure in ["network", "storage", "bad_edit"] {
        let mut f = Fixture::new(LarkRegion::Lark, false, false).await;
        let reference = f.confirmed(true).await;
        if failure == "network" {
            f.api.uncertain.store(true, Ordering::SeqCst);
        }
        if failure == "storage" {
            f.api.fail_receipt.store(true, Ordering::SeqCst);
        }
        if failure == "bad_edit" {
            f.api.overrides.lock().unwrap().insert(
                "PUT /open-apis/im/v1/messages/om_sent".into(),
                json!({"code":0,"data":{}}),
            );
        }
        let result = f
            .app
            .state
            .update(
                f.app.caller(),
                &reference,
                serde_json::from_value(json!({"text":"new text"})).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.delivery.kind(), "unknown", "{failure}");
        f.api.uncertain.store(false, Ordering::SeqCst);
        f.api.fail_receipt.store(false, Ordering::SeqCst);
        f.app.backend.fail_put.store(false, Ordering::SeqCst);
        f.app.restart();
        assert_eq!(
            f.app
                .state
                .delete_message(f.app.caller(), &reference, "delete")
                .await
                .unwrap()
                .delivery
                .kind(),
            "unknown"
        );
        assert_eq!(
            f.react(&reference, "reaction", Some("SMILE"))
                .await
                .unwrap()
                .delivery
                .kind(),
            "unknown"
        );
        assert_eq!(f.writes().len(), 1);
    }
}
#[tokio::test]
async fn native_lark_concurrent_keys_share_one_exact_message_fence() {
    let f = Fixture::new(LarkRegion::Feishu, true, false).await;
    let reference = f.received().await;
    f.api.delay_ms.store(50, Ordering::SeqCst);
    let (a, b) = tokio::join!(
        f.react(&reference, "a", Some("SMILE")),
        f.react(&reference, "b", Some("THUMBSUP"))
    );
    let outcomes = [a.unwrap().delivery.kind(), b.unwrap().delivery.kind()];
    assert!(outcomes.contains(&"confirmed") && outcomes.contains(&"unknown"));
    assert_eq!(f.writes().len(), 1);
}
#[tokio::test]
async fn native_lark_reaction_uncertainty_and_partial_rejection_never_resend() {
    for failure in ["network", "storage", "replacement_rejected"] {
        let mut f = Fixture::new(LarkRegion::Feishu, true, false).await;
        let reference = f.received().await;
        if failure == "replacement_rejected" {
            assert_eq!(
                f.react(&reference, "first", Some("SMILE"))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "confirmed"
            );
            f.api.overrides.lock().unwrap().insert(
                "POST /open-apis/im/v1/messages/om_received/reactions".into(),
                json!({"code":231017}),
            );
        } else if failure == "network" {
            f.api.uncertain.store(true, Ordering::SeqCst);
        } else {
            f.api.fail_receipt.store(true, Ordering::SeqCst);
        }
        assert_eq!(
            f.react(&reference, "uncertain", Some("THUMBSUP"))
                .await
                .unwrap()
                .delivery
                .kind(),
            "unknown"
        );
        f.api.uncertain.store(false, Ordering::SeqCst);
        f.api.fail_receipt.store(false, Ordering::SeqCst);
        f.app.backend.fail_put.store(false, Ordering::SeqCst);
        f.api.overrides.lock().unwrap().clear();
        f.app.restart();
        for key in ["uncertain", "new-key"] {
            assert_eq!(
                f.react(&reference, key, Some("THUMBSUP"))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unknown"
            );
        }
        assert_eq!(
            f.writes().len(),
            if failure == "replacement_rejected" {
                3
            } else {
                1
            }
        );
    }
}
