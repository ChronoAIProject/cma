use super::{
    super::{
        bridge_invokes_tests::fixture as invokes, bridge_source_controls_tests::fixture,
        resource_access_tests::context,
    },
    Scenario,
};
use axum::{
    http::{Method, StatusCode},
    Extension,
};
use layer2_core::bridge::invoke::*;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("source_consent_") {
        return None;
    }
    let f = fixture::fixture().await;
    if name.contains("_revoked_invoke") {
        let enabled = invokes::send(
            &f.router,
            Method::PUT,
            &f.path,
            "owner-a",
            Some("seed-enable"),
            Some(fixture::body(0, "allow_sender_cancel_and_steer")),
        )
        .await;
        assert_eq!(enabled.0, StatusCode::OK);
        f.bridge
            .service
            .revoke(
                &context("owner-a"),
                None,
                RevokeBridgeInvokeGrant {
                    grant_id: BridgeInvokeGrantId(f.grant.clone()),
                    expected_bridge_revision: 2,
                    expected_revision: 1,
                    idempotency_key: "seed-revoke-invoke".into(),
                },
            )
            .await
            .unwrap();
    }
    let subject = if name.contains("_outsider") {
        "outsider"
    } else {
        "owner-a"
    };
    let mut caller = context(subject);
    if name.contains("_human_actor") {
        caller.actor = layer2_core::resource_access::ResourceActor::Human {
            subject: caller.caller.subject.clone(),
        };
    }
    let router = if name.contains("_unconfigured") {
        super::super::bridge_source_control_router(None)
    } else {
        f.router
    };
    Some(Scenario {
        router: router.layer(Extension(caller.clone())).layer(Extension(
            crate::DelegatedCredential::new("request-credential".into()),
        )),
        subject: if name.contains("_anonymous") {
            None
        } else {
            Some(subject)
        },
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}
