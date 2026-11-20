use super::transport::ToolTransport;
use super::*;
use crate::ingress_budget::{
    AdmissionRefusal, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy, OwnedWork, SharedOwned,
};

impl ToolTransport {
    pub(in crate::hosted::managed_context) fn clone_admitted(
        &self,
        policy: &IngressOwnershipPolicy,
    ) -> Result<Self, AdmittedError> {
        match self {
            Self::Access(binding) => Ok(Self::AdmittedAccess(binding.clone_admitted(policy)?)),
            Self::Provider(binding) => Ok(Self::AdmittedProvider(binding.clone_admitted(policy)?)),
            Self::AdmittedAccess(binding) if binding.belongs_to(policy) => {
                Ok(Self::AdmittedAccess(binding.clone()))
            }
            Self::AdmittedProvider(binding) if binding.belongs_to(policy) => {
                Ok(Self::AdmittedProvider(binding.clone()))
            }
            _ => Err(AdmissionRefusal::InvalidPlan {
                phase: "installed tool transport ownership",
            }
            .into()),
        }
    }
}

impl HostedContextRefresh {
    pub(in crate::hosted) async fn prepare_transport_admitted(
        &self,
        session: &str,
        sequence: u64,
        origin: Option<&str>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<Option<ToolTransport>, AdmittedError> {
        self.require_owner(policy)?;
        let Some(origin) = origin else {
            return Ok(None);
        };
        let installed = self
            .0
            .installed_transport
            .read()
            .expect("transport state lock")
            .as_ref()
            .map(|value| value.clone_admitted(policy))
            .transpose()?;
        let result = match (&self.0.access, &self.0.provider, installed.as_ref()) {
            (Some((source, binding)), None, None) => {
                let result = complete(
                    source.prepare_transport_admitted(binding, origin, session, sequence, policy),
                    policy,
                )
                .await;
                result
                    .and_then(|value| share(value, policy))
                    .map(ToolTransport::AdmittedAccess)
            }
            (Some((source, _)), None, Some(ToolTransport::AdmittedAccess(binding))) => {
                let result = complete(
                    source.prepare_transport_admitted(binding, origin, session, sequence, policy),
                    policy,
                )
                .await;
                result
                    .and_then(|value| share(value, policy))
                    .map(ToolTransport::AdmittedAccess)
            }
            (None, Some((source, binding)), None) => {
                let result = complete(
                    source.prepare_transport_admitted(binding, origin, session, sequence, policy),
                    policy,
                )
                .await;
                result
                    .and_then(|value| share(value, policy))
                    .map(ToolTransport::AdmittedProvider)
            }
            (None, Some((source, _)), Some(ToolTransport::AdmittedProvider(binding))) => {
                let result = complete(
                    source.prepare_transport_admitted(binding, origin, session, sequence, policy),
                    policy,
                )
                .await;
                result
                    .and_then(|value| share(value, policy))
                    .map(ToolTransport::AdmittedProvider)
            }
            _ => Err(AdmittedError::Unsupported(
                "conversation has no unique tool transport authority",
            )),
        };
        result.map(Some).map_err(|error| {
            if matches!(error.as_adapter(), Some(AdapterError::Rejected(_))) {
                AdmittedError::Unsupported("retained tool transport authority refused maintenance")
            } else {
                error
            }
        })
    }

    pub(in crate::hosted) async fn confirm_transport_admitted(
        &self,
        transport: &ToolTransport,
        session: &str,
        sequence: u64,
        policy: &IngressOwnershipPolicy,
    ) -> Result<(), AdmittedError> {
        self.require_owner(policy)?;
        let installed = transport.clone_admitted(policy)?;
        let operation = match (&installed, &self.0.access, &self.0.provider) {
            (ToolTransport::AdmittedAccess(binding), Some((source, _)), None) => {
                source.confirm_transport_admitted(binding, session, sequence, policy)
            }
            (ToolTransport::AdmittedProvider(binding), None, Some((source, _))) => {
                source.confirm_transport_admitted(binding, session, sequence, policy)
            }
            _ => {
                return Err(AdmittedError::Unavailable(
                    "tool confirmation source is unavailable",
                ))
            }
        };
        complete(operation, policy).await?;
        *self
            .0
            .installed_transport
            .write()
            .expect("transport state lock") = Some(installed);
        Ok(())
    }
}

async fn complete<T>(
    operation: Result<AdmittedOperation<'_, T>, AdmittedError>,
    policy: &IngressOwnershipPolicy,
) -> Result<OwnedWork<T>, AdmittedError> {
    let operation = operation?;
    if !operation.belongs_to(policy) {
        return Err(AdmissionRefusal::InvalidPlan {
            phase: "tool source ownership",
        }
        .into());
    }
    let result = operation.await?;
    if !result.belongs_to(policy) {
        return Err(AdmissionRefusal::InvalidPlan {
            phase: "tool result ownership",
        }
        .into());
    }
    Ok(result)
}

fn share<T>(
    value: OwnedWork<T>,
    policy: &IngressOwnershipPolicy,
) -> Result<SharedOwned<T>, AdmittedError> {
    value
        .work_scope()
        .expect("admitted tool binding")
        .admit_extra(
            policy,
            IngressClass::Data,
            AllocationPlan::checked(
                "retained tool binding owner",
                &[size_of::<T>(), 2 * size_of::<usize>(), align_of::<T>()],
            )?,
        )?;
    Ok(value.into_shared_admitted()?)
}

#[cfg(test)]
#[path = "transport_admission_tests.rs"]
pub(in crate::hosted) mod tests;
