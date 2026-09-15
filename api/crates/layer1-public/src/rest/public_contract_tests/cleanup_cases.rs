use super::{super::bridge_cleanup_tests::fixture, Scenario};
use axum::Extension;
use std::sync::atomic::Ordering;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("bridge_cleanup_") {
        return None;
    }
    let f = fixture::fixture().await;
    let subject = if name.contains("_outsider") {
        "outsider"
    } else {
        "owner-a"
    };
    let mut caller = fixture::context(subject);
    if name.contains("_api_actor") {
        caller.actor = layer2_core::resource_access::ResourceActor::Api {
            subject: caller.caller.subject.clone(),
            delegation_id: None,
        };
    }
    for (suffix, failure) in [("_unavailable", 1), ("_corrupt", 2), ("_conflict", 3)] {
        if name.ends_with(suffix) {
            f.memory.failure.store(failure, Ordering::SeqCst)
        }
    }
    let router = if name.contains("_unconfigured") {
        super::super::bridge_cleanup_router(None)
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
