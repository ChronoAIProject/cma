use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::{
    Authorization, Capability, OperationClaim, OperationReservation, OperationStore, StoreError,
};

use super::super::{
    agents_tests as agents, instances_tests as instances, profiles_tests as profiles,
};
use super::Scenario;

mod dependencies;
mod exhausted_revision;
mod instance_store;
mod operator_conflict;
mod profile_store;
mod scanner;
mod settings_store;

enum OperationFault {
    Pending,
    Recovery,
    Unavailable,
}

#[async_trait]
impl OperationStore for OperationFault {
    async fn reserve(&self, _: &OperationClaim) -> Result<OperationReservation, StoreError> {
        match self {
            Self::Pending => Ok(OperationReservation::Pending),
            Self::Recovery => Ok(OperationReservation::RecoveryRequired),
            Self::Unavailable => Err(StoreError::Unavailable(
                "contract operation store unavailable".into(),
            )),
        }
    }

    async fn complete(&self, _: &OperationClaim, _: Vec<u8>) -> Result<(), StoreError> {
        panic!("a refused operation reservation must never execute or record a response")
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if let Some(scenario) = exhausted_revision::scenario(name) {
        return Some(scenario);
    }
    if let Some(scenario) = operator_conflict::scenario(name) {
        return Some(scenario);
    }
    if let Some(scenario) = scanner::scenario(name) {
        return Some(scenario);
    }
    if let Some(scenario) = dependencies::scenario(name).await {
        return Some(scenario);
    }
    if name == "profiles_store_unavailable" {
        return Some(Scenario {
            router: profiles::fixture_with_profile_store(Arc::new(
                profile_store::UnavailableProfiles,
            )),
            subject: Some("contract-owner"),
            authorization: Authorization::new([
                Capability::ManageOwnProfiles,
                Capability::UseOwnSandboxes,
            ]),
            symbols: Default::default(),
        });
    }
    let (domain, suffix) = name.split_once("_operation_")?;
    let fault = Arc::new(match suffix {
        "pending" => OperationFault::Pending,
        "recovery" => OperationFault::Recovery,
        "unavailable" => OperationFault::Unavailable,
        _ => return None,
    });
    let (router, subject, capabilities) = match domain {
        "profiles" => (
            profiles::fixture_with_operations(fault),
            "contract-owner",
            vec![Capability::ManageOwnProfiles, Capability::UseOwnSandboxes],
        ),
        "instances" => (
            instances::fixture_with_operations(10, Some(fault)).router,
            "owner-a",
            vec![
                Capability::CreateSandbox,
                Capability::UseOwnSandboxes,
                Capability::DeleteOwnSandbox,
            ],
        ),
        "agents" => (
            agents::fixture_with_operations(fault).await.router,
            "owner-a",
            vec![Capability::UseOwnSandboxes],
        ),
        _ => return None,
    };
    Some(Scenario {
        router,
        subject: Some(subject),
        authorization: Authorization::new(capabilities),
        symbols: Default::default(),
    })
}
