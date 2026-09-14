use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::profiles::{EnvironmentVariable, PublicValueScanError, PublicValueScanner};
use layer2_core::{Authorization, Capability};

use super::super::super::profiles_tests;
use super::super::Scenario;

#[derive(Debug)]
struct FailingScanner(AtomicUsize);

#[async_trait]
impl PublicValueScanner for FailingScanner {
    async fn scan(&self, _: &[EnvironmentVariable]) -> Result<(), PublicValueScanError> {
        self.0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .map(|_| ())
            .map_err(|_| PublicValueScanError::Unavailable)
    }
}

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let permitted = match name {
        "profiles_scanner_unavailable" => 0,
        "profiles_scanner_later_unavailable" => 1,
        _ => return None,
    };
    Some(Scenario {
        router: profiles_tests::fixture_with_public_scanner(Arc::new(FailingScanner(
            AtomicUsize::new(permitted),
        ))),
        subject: Some("contract-owner"),
        authorization: Authorization::new([Capability::ManageOwnProfiles]),
        symbols: Default::default(),
    })
}
