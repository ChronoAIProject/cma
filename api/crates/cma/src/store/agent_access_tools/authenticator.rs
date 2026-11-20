use super::{AuthenticatedAgentAccess, MongoAgentAccessToolSource};
use layer2_core::sandbox::agent_access::AgentAccessError;
use layer2_core::sandbox::agent_access_tool::AgentAccessToolAuthenticator;
use layer3_adapter::AdapterError;

#[async_trait::async_trait]
impl AgentAccessToolAuthenticator for MongoAgentAccessToolSource {
    async fn authenticate_setup(&self, capability: &str) -> Result<(), AgentAccessError> {
        MongoAgentAccessToolSource::authenticate_setup(self, capability)
            .await
            .map_err(map)
    }

    async fn authenticate(
        &self,
        capability: &str,
    ) -> Result<AuthenticatedAgentAccess, AgentAccessError> {
        MongoAgentAccessToolSource::authenticate(self, capability)
            .await
            .map_err(map)
    }
}

fn map(error: AdapterError) -> AgentAccessError {
    match error {
        AdapterError::AccessRevoked => AgentAccessError::AccountDeletionInProgress,
        AdapterError::Unavailable(_) => AgentAccessError::Unavailable,
        _ => AgentAccessError::Forbidden,
    }
}

#[test]
fn account_deletion_is_a_typed_authentication_refusal() {
    assert_eq!(
        map(AdapterError::AccessRevoked.context("current owner authority")),
        AgentAccessError::AccountDeletionInProgress,
    );
    assert_eq!(
        map(AdapterError::Rejected(
            "account_deletion_in_progress".into()
        )),
        AgentAccessError::Forbidden,
        "diagnostic text does not establish the refusal category",
    );
}
