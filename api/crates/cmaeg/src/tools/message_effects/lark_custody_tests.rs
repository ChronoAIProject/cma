use super::lark_fixture::*;
use crate::{auth::AgentCaller, channels::routing_fixture as routing};
use cmaeg_core::channel::Platform;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn lark_named_consumer_uses_its_provider_and_never_borrows_other_keys() {
    let mut f = App::new(Platform::Lark).await;
    let default = f.caller();
    f.named_consumer();
    let reference = f.reference().await;
    assert_eq!(
        reference.open(&f.state.keys).unwrap().payload.consumer,
        Some(cmaeg_core::ids::ConsumerId::parse(routing::REVIEWER).unwrap())
    );
    let seen = f.provider.seen();
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|request| {
        request.uri.contains("/bindings/binding-reviewer/")
            && request.bearer == format!("Bearer {}", f.key)
    }));
    let other = AgentCaller {
        key_hash: Sha256::digest(routing::agent_key(routing::NYX).as_bytes()).into(),
    };
    for caller in [default, other] {
        assert!(f
            .state
            .delete_message(caller, &reference, "wrong-consumer")
            .await
            .is_err());
    }
    assert!(f.writes().is_empty());
    assert_eq!(
        f.state
            .delete_message(f.caller(), &reference, "correct-consumer")
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
}

#[tokio::test]
async fn lark_named_consumer_revocation_also_prevents_confirmed_receipt_replay() {
    for changed in ["binding", "key", "removed", "pending", "credential"] {
        let mut f = App::new(Platform::Lark).await;
        f.named_consumer();
        let reference = f.reference().await;
        assert_eq!(
            f.state
                .delete_message(f.caller(), &reference, "one")
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
        let routing = f.definition.routing.as_mut().unwrap();
        match changed {
            "binding" => routing.consumers[0].provider.binding_id = "replacement".parse().unwrap(),
            "key" => {
                routing.consumers[0].agent_key_hash = routing::hash("rotated-key");
                f.definition.secrets.consumer_keys[0].1 =
                    cmaeg_core::channel::Secret::new("rotated-key");
            }
            "removed" => {
                routing.consumers.remove(0);
                f.definition.secrets.consumer_keys.remove(0);
            }
            "pending" => routing.consumers[0].pending = true,
            "credential" => {
                f.definition.secrets.consumer_keys.remove(0);
            }
            _ => unreachable!(),
        }
        f.replace();
        for key in ["one", "fresh"] {
            assert!(
                f.state
                    .delete_message(f.caller(), &reference, key)
                    .await
                    .is_err(),
                "{changed}"
            );
        }
        assert_eq!(f.writes().len(), 1, "{changed}");
    }
}

#[tokio::test]
async fn lark_retained_effect_is_callable_after_discovery_is_disabled() {
    let mut f = App::new(Platform::Lark).await;
    let reference = f.reference().await;
    f.state.lark_effects_issuance_enabled = false;
    assert!(!crate::mcp::catalog::view(&f.state)
        .iter()
        .any(|tool| tool.name == "deleteMessage"));
    assert_eq!(
        f.state
            .delete_message(f.caller(), &reference, "retained")
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    assert_eq!(f.writes().len(), 1);
}
