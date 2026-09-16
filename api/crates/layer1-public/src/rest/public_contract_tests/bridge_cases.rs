use super::{
    super::{bridges_tests::fixture, resource_access_tests::context},
    Scenario,
};
use axum::Extension;
use std::sync::atomic::Ordering;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("bridge_setup_") {
        return None;
    }
    let (router, _, ports) = fixture::fixture();
    let org = name.contains("_org_");
    if name.contains("_fixed_") {
        let mut body = fixture::create_body(false);
        body["config"]["partition"] = serde_json::json!("fixed_shared_agent");
        body["config"]["placement"] = serde_json::json!({"kind":"existing_agent","target":{"sandbox_id":"sbx_target","workspace_id":"wks_target","worktree_id":"wkt_target","agent_id":"agt_target"}});
        assert_eq!(
            fixture::send(
                &router,
                axum::http::Method::POST,
                "/api/v1/bridges",
                "owner",
                Some("create-one"),
                Some(body)
            )
            .await
            .0,
            axum::http::StatusCode::CREATED
        );
    } else if !name.contains("_empty_") && !name.contains("_unconfigured_") {
        fixture::create(&router, org).await;
    }
    if name.contains("_member_") {
        ports.role.store(1, Ordering::SeqCst);
    }
    if name.contains("_missing_") {
        ports.role.store(0, Ordering::SeqCst);
    }
    if name.contains("_outage_") {
        ports.role.store(3, Ordering::SeqCst);
    }
    if name.contains("_verified_") {
        ports.validation.store(1, Ordering::SeqCst);
    }
    let subject = if name.contains("_outsider_") {
        "outsider"
    } else {
        "owner"
    };
    let mut router = if name.contains("_unconfigured_") {
        super::super::bridge_router(None)
    } else {
        router
    };
    let caller = context(subject);
    router = router.layer(Extension(if name.contains("_invalid_context_") {
        context("another")
    } else {
        caller.clone()
    }));
    if !name.contains("_no_credential_") {
        router = router.layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    }
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
