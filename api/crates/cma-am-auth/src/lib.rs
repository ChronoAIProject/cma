//! Agent Manager authentication and permission mapping.

pub mod actor_links;
mod api_bearer;
pub mod auth;
pub mod diagnostic_format;
pub mod diagnostic_request;
pub mod diagnostics;
pub mod permissions;
pub mod profile;
pub mod provider;
mod proxy_grant;
mod records;
pub mod request_observation;
pub mod service_config;
pub mod shutdown;
mod store;
pub mod token;

pub use api_bearer::ApiBearerCustody;
pub use records::{
    next_auth_counter, BrowserLogin, DurableSubjectTokens, SecretDigest, SecretValue,
    SubjectAuthState, SubjectRefreshLease,
};

pub use proxy_grant::ProxyGrant;
pub use store::{AuthStore, LoginAttempt, RecoveryIntent};

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod dependency_test_support;
