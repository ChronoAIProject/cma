//! Locate the exact consent which committed the current credential scope.
use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox::Sandbox;

pub(super) fn valid_owner_authorization(value: &AgentAuthorizationRevision) -> bool {
    if value.policy_revision == 0
        || value.grant_revision.is_some()
        || value.authority_id.chars().any(char::is_control)
    {
        return false;
    }
    if let Some(id) = value.authority_id.strip_prefix("personal:") {
        return !id.is_empty();
    }
    value
        .authority_id
        .strip_prefix("owner-api:")
        .and_then(|id| id.split_once(':'))
        .is_some_and(|(id, expiry)| {
            id.len() == 64
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                && expiry.parse::<u64>().is_ok_and(|seconds| seconds > 0)
        })
}

impl MongoSandboxStore {
    pub(crate) async fn credential_scope_authorization(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<AgentAuthorizationRevision>, AgentAccessError> {
        let floor = sandbox.credential.service_access_generation_floor();
        if floor == 0 {
            return Ok(None);
        }
        let floor = i64::try_from(floor).map_err(|_| AgentAccessError::Invalid)?;
        let rows: Vec<_> = self
            .agent_access_requests
            .find(doc! {
                "record.requester.sandbox_id":sandbox.id.as_str(),
                "record.progress.grant_generation_floor":floor,
            })
            .limit(2)
            .await
            .map_err(|_| AgentAccessError::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| AgentAccessError::Unavailable)?;
        let value = match rows.as_slice() {
            [] => return Ok(None), // Historical explicit grants use owner login custody.
            [value] => value,
            _ => return Err(AgentAccessError::Invalid),
        };
        let record = read(value)?;
        if record.requester.owner != sandbox.owner
            || record.requester.sandbox_id != sandbox.id
            || record.approved_services()? != sandbox.credential.effective_reaches()
        {
            return Err(AgentAccessError::Invalid);
        }
        let Some(AgentAccessDecision::Approved { authorization, .. }) = record.decision else {
            return Err(AgentAccessError::Forbidden);
        };
        Ok(Some(authorization))
    }
}
