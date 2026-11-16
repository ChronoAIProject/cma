//! Route the original delegated operation, never a synthetic trusted context.
use super::*;
use layer2_core::bots::BotError;

pub(super) fn delegated_mutation(method: &Method, uri: &Uri) -> bool {
    method == Method::POST && uri.path() == "/api/v1/agent-delegations/operations"
}
impl PublicAccess {
    pub(super) async fn delegated_target(
        &self,
        request: RedemptionRequest<'_>,
    ) -> Result<Option<AgentId>, StatusCode> {
        let setup = self.setup.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        let credential = request.credential.ok_or(StatusCode::UNAUTHORIZED)?;
        self.auth
            .verify_owner_admission_origin(
                request.context,
                credential,
                request.method,
                request.uri,
                request.headers,
            )
            .await
            .map_err(|_| StatusCode::FORBIDDEN)?;
        let Some(target) = setup
            .owner_delete_target(credential, request.body)
            .await
            .map_err(error)?
        else {
            return Ok(None);
        };
        let resource = ResourceTarget::Agent {
            agent_id: target.agent_id.clone(),
        };
        let authorized = self
            .access
            .authorize(
                &target.context,
                Some(target.credential.expose()),
                &resource,
                ResourceCapability::OperateAgent,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await
            .map_err(access_error)?;
        self.access
            .validate_read_result(&authorized, &resource, ResourceCapability::OperateAgent)
            .await
            .map_err(access_error)?;
        self.reads
            .lineage(&target.context.caller, &target.agent_id)
            .await
            .map_err(read_error)?;
        let current = setup
            .owner_delete_target(credential, request.body)
            .await
            .map_err(error)?
            .ok_or(StatusCode::FORBIDDEN)?;
        if current.agent_id != target.agent_id
            || current.context != target.context
            || current.credential != target.credential
        {
            return Err(StatusCode::FORBIDDEN);
        }
        self.auth
            .verify_owner_admission_origin(
                request.context,
                credential,
                request.method,
                request.uri,
                request.headers,
            )
            .await
            .map_err(|_| StatusCode::FORBIDDEN)?;
        Ok(Some(target.agent_id))
    }
}
fn error(error: BotError) -> StatusCode {
    match error {
        BotError::Forbidden => StatusCode::FORBIDDEN,
        BotError::NotFound => StatusCode::NOT_FOUND,
        BotError::Conflict => StatusCode::CONFLICT,
        BotError::Invalid(_) => StatusCode::BAD_REQUEST,
        BotError::ConnectionRequired { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        BotError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}
