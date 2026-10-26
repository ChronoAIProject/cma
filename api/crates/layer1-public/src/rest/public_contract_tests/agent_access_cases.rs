use std::collections::BTreeMap;

use axum::Extension;
use layer2_core::sandbox::agent_access::AgentAccessError;

use super::Scenario;
use crate::rest::{agent_access_requests_tests, resource_access_tests::context};

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let error = match name {
        "agent_access_owner" | "agent_access_anonymous" | "agent_access_no_credential" => None,
        "agent_access_invalid" => Some(AgentAccessError::Invalid),
        "agent_access_forbidden" => Some(AgentAccessError::Forbidden),
        "agent_access_not_found" => Some(AgentAccessError::NotFound),
        "agent_access_conflict" => Some(AgentAccessError::Conflict),
        "agent_access_unavailable" => Some(AgentAccessError::Unavailable),
        _ => return None,
    };
    let (router, service) = agent_access_requests_tests::app();
    *service.error.lock().unwrap() = error;
    let caller = context("owner");
    let mut router = router.layer(Extension(caller.clone()));
    if name != "agent_access_no_credential" {
        router = router.layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    }
    Some(Scenario {
        router,
        subject: (name != "agent_access_anonymous").then_some("owner"),
        authorization: caller.caller.authorization,
        symbols: BTreeMap::new(),
    })
}
