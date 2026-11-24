//! Agent Manager authentication and permission mapping.

pub mod actor_links;
mod api_bearer;
pub mod permissions;
pub mod profile;
mod proxy_grant;
mod records;
mod store;
pub mod token;

pub use api_bearer::ApiBearerCustody;
pub use records::{
    next_auth_counter, BrowserLogin, DurableSubjectTokens, SecretDigest, SecretValue,
    SubjectAuthState, SubjectRefreshLease,
};

pub use proxy_grant::ProxyGrant;
pub use store::{AuthStore, LoginAttempt, RecoveryIntent};
