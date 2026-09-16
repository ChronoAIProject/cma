use super::{
    super::{bridge_invokes_tests::fixture, resource_access_tests::context},
    Scenario,
};
use axum::Extension;
use layer2_core::bridge::{invoke::*, BridgeId};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("bridge_invoke_") {
        return None;
    }
    let f = fixture::fixture().await;
    if name.contains("_issued_") || name.contains("_revoked_") {
        let mut request: IssueBridgeInvokeGrant = serde_json::from_value(serde_json::json!({
            "bridge_id":f.id,"expected_bridge_revision":1,"target":fixture::issue_body()["target"],
            "shared_context":"use_retained_agent_context","idempotency_key":"fixture-consent"
        }))
        .unwrap();
        request.bridge_id = BridgeId(f.id.clone());
        let receipt = f
            .service
            .issue(&context("owner-a"), None, request)
            .await
            .unwrap();
        if name.contains("_revoked_") {
            f.service
                .revoke(
                    &context("owner-a"),
                    None,
                    RevokeBridgeInvokeGrant {
                        grant_id: receipt.result.id,
                        expected_bridge_revision: 1,
                        expected_revision: 1,
                        idempotency_key: "fixture-revoke".into(),
                    },
                )
                .await
                .unwrap();
        }
    }
    let subject = if name.contains("_outsider_") {
        "outsider"
    } else {
        "owner-a"
    };
    let caller = context(subject);
    let router = if name.contains("_unconfigured_") {
        super::super::bridge_invoke_router(None)
    } else {
        f.router
    };
    let router =
        router
            .layer(Extension(caller.clone()))
            .layer(Extension(crate::DelegatedCredential::new(
                "request-credential".into(),
            )));
    Some(Scenario {
        router,
        subject: if name.contains("_anonymous_") {
            None
        } else {
            Some(subject)
        },
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}
