//! Platform spend scenarios share the route tests' ledger and write gates.

use super::Scenario;
use crate::rest::admin::tests::{fixture, spend_test_support::*};
use layer2_core::{Authorization, Capability};

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("admin_spend_")?;
    let router = if mode == "absent" {
        fixture(false).app
    } else {
        spend_fixture_with(
            mode != "writes_closed",
            if mode == "closed" {
                Ledger::closed()
            } else {
                Ledger {
                    unmetered: mode == "off",
                    unavailable: mode == "unavailable",
                    ..Ledger::default()
                }
            },
        )
        .app
    };
    Some(Scenario {
        router,
        subject: (mode != "anonymous").then_some("operator-spend-fixture"),
        authorization: if mode == "forbidden" {
            Authorization::none()
        } else {
            Authorization::new([Capability::ConfigurePlatform])
        },
        symbols: Default::default(),
    })
}
