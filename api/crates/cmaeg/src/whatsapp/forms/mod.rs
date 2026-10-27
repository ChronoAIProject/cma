//! Static, managed Flow assets and explicit prepare/status/commit authority.
pub(crate) mod callback;
mod commit;
pub(crate) mod compile;
pub(crate) mod management;
pub(crate) mod provision;
pub(crate) mod registry;
pub(crate) mod service;
#[cfg(test)]
mod test_fixture;
#[cfg(test)]
mod tests;
mod unique_json;
pub(crate) mod values;
pub(crate) mod worker;

pub(crate) const JSON_VERSION: &str = "7.3";
pub(crate) const SCREEN: &str = "CMA_FORM";
pub(crate) const MAX_JSON_BYTES: usize = 64 * 1024;
pub(crate) const MAX_SUBMITTED_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    Invalid,
    Unavailable,
    Conflict,
    Revoked,
    Pending,
    Missing,
}

pub(crate) fn hash(value: &impl serde::Serialize) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("Flow facts serialize"))
    )
}

#[cfg(test)]
mod management_tests;

#[cfg(test)]
pub(crate) mod durable_tests;
