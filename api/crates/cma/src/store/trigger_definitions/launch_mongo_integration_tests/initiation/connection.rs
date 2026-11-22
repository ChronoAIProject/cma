use super::*;

pub(super) struct Connected;
#[async_trait::async_trait]
impl ServiceConnectionBroker for Connected {
    async fn requirements(
        &self,
        _: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        Ok(services
            .iter()
            .map(|service| ServiceRequirement {
                service: service.clone(),
                state: ServiceRequirementState::Satisfied,
            })
            .collect())
    }
    async fn create_handoff(
        &self,
        _: &SubjectId,
        _: &ServiceId,
        _: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        Err(ServiceConnectionError::Refused)
    }
    async fn handoff_state(
        &self,
        _: &SubjectId,
        _: &str,
        _: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        Err(ServiceConnectionError::Refused)
    }
}
