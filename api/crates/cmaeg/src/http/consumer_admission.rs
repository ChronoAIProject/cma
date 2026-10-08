//! Public admission results for each target of one routed event.
use cmaeg_core::ids::{ConsumerId, ConsumerName, ConversationId};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConsumerAdmission {
    pub consumer_id: ConsumerId,
    pub name: ConsumerName,
    pub conversation_id: ConversationId,
    pub status: AdmissionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionStatus {
    Accepted,
    Deferred,
    Refused,
}

impl TryFrom<crate::turns::fanout::ConsumerLaunch> for ConsumerAdmission {
    type Error = cmaeg_core::ValidationError;

    fn try_from(launch: crate::turns::fanout::ConsumerLaunch) -> Result<Self, Self::Error> {
        use crate::ingress::launch::LaunchOutcome;
        let (status, code, provider_code) = match launch.outcome {
            LaunchOutcome::Accepted { .. } => (AdmissionStatus::Accepted, None, None),
            LaunchOutcome::Deferred => (AdmissionStatus::Deferred, None, None),
            LaunchOutcome::Refused(failure) => (
                AdmissionStatus::Refused,
                Some(failure.code.to_owned()),
                crate::delivery::provider_code(failure.provider_code),
            ),
        };
        Ok(Self {
            consumer_id: launch.id,
            name: ConsumerName::parse(&launch.name)?,
            conversation_id: launch.conversation_id,
            status,
            code,
            provider_code,
        })
    }
}
