use super::{
    super::{bridge_outbox_tests::fixture, resource_access_tests::context},
    Scenario,
};
use axum::Extension;
pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("bridge_outbox_") {
        return None;
    }
    let f = fixture::fixture().await;
    let subject = if name.contains("_outsider_") {
        "outsider"
    } else {
        "owner-a"
    };
    let caller = context(subject);
    let router = if name.contains("_unconfigured_") {
        super::super::bridge_outbox_router(None)
    } else {
        f.router
    };
    let router = router.layer(Extension(caller.clone()));
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
