//! Agent Manager composition and application authentication contracts.

pub(crate) use cma_am_auth::auth::*;

#[cfg(test)]
pub(crate) mod desktop_test_support;

#[cfg(test)]
pub(crate) mod account_standing;

#[cfg(test)]
pub(crate) mod registration {
    pub(crate) use cma_am_auth::auth::registration::*;
    mod mongo_tests;
}

#[cfg(test)]
mod desktop {
    use super::{AuthMode, AuthState, RequestAuth, AUTH_COOKIE};
    use axum::http::HeaderMap;
    use cma_am_auth::auth::test_support::capture_desktop as capture;
    use layer2_core::{
        resource_access::CredentialSource,
        sandbox::{DesktopAuthorization, DesktopError},
        Caller, Capability, SubjectId,
    };
    use std::{sync::Arc, time::SystemTime};
    mod tests;
}

#[cfg(test)]
mod agent_keys {
    use super::{AuthMode, AuthState, ProviderMetadata};
    use cma_am_auth::auth::test_support::is_agent_key;
    use std::time::{Duration, SystemTime};
    mod tests;
}

#[cfg(test)]
mod cookie_authorization;

#[cfg(test)]
mod refresh;

#[cfg(test)]
mod resource_request {
    use super::{AuthMode, AuthState};
    use axum::{
        body::Body,
        extract::Request,
        http::{header, StatusCode},
        middleware::Next,
        Json,
    };
    use cma_am_auth::auth::test_support::request_descriptor;
    pub(super) use cma_am_auth::auth::test_support::RESOURCE_REQUEST_HEADER as HEADER;
    use layer2_core::{
        resource_access::{CallerContext, CredentialContext, CredentialSource, ResourceActor},
        Caller,
    };
    use std::{sync::Arc, time::SystemTime};
    mod tests;
}

#[cfg(test)]
pub(crate) mod service_authorization {
    pub(crate) use cma_am_auth::auth::service_authorization::*;

    use super::{
        persistence::{SecretDigest, SecretValue},
        token::random_token,
        AuthMode, AuthState, AUTH_COOKIE_DOMAIN,
    };
    use axum::{
        http::{header, StatusCode},
        response::Response,
    };
    use cma_am_auth::auth::test_support::authorization_code_challenge as code_challenge;
    use layer2_core::{resource_access::CredentialSource, ServiceId, StoreError, SubjectId};
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
        time::{Duration, SystemTime},
    };
    use url::Url;

    mod tests;

    mod nyxid {
        use super::*;
        mod tests;
    }
}

#[cfg(test)]
mod worker_credentials {
    use super::{AuthMode, AuthState};
    use axum::{
        http::{header, StatusCode},
        response::Response,
    };
    use cma_am_auth::auth::test_support::matches_worker_credentials_path as matches_path;
    use layer1_public::rest::worker_credentials::WORKER_CLASSIFIER_PATH;
    use layer2_core::SubjectId;
    use serde_json::Value;
    mod tests;
}

#[cfg(all(test, unix))]
pub(crate) mod keys {
    pub(crate) use cma_am_auth::auth::keys::*;

    use super::persistence::SecretValue;
    use layer2_core::ServiceId;
    use std::time::Duration;

    mod fixture_contract_tests;
}
