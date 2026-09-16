use super::{super::bridge_partitions_tests::fixture, Scenario};
use axum::Extension;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("bridge_partitions_") {
        return None;
    }
    let f = fixture::fixture_for(false, name.ends_with("_redacted")).await;
    let subject = if name.ends_with("_outsider") {
        "outsider"
    } else {
        "owner-a"
    };
    let caller = fixture::context(subject);
    let router = if name.ends_with("_unconfigured") {
        super::super::bridge_partitions_router(None)
    } else {
        f.router
    };
    Some(Scenario {
        router: router.layer(Extension(caller.clone())).layer(Extension(
            crate::DelegatedCredential::new("request-credential".into()),
        )),
        subject: Some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}
