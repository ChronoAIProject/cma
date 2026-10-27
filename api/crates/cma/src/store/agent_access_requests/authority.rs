//! Locate the exact consent which committed or renewed the current scope.
use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox::{Sandbox, SandboxStatus};
use std::collections::{BTreeMap, BTreeSet};

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

fn scope_filter(sandbox: &Sandbox) -> Result<mongodb::bson::Document, AgentAccessError> {
    let floor = i64::try_from(sandbox.credential.service_access_generation_floor())
        .map_err(|_| AgentAccessError::Invalid)?;
    Ok(doc! {
        "record.requester.sandbox_id":sandbox.id.as_str(),
        "record.progress.grant_generation_floor":floor,
    })
}

// A unique linked chain makes selection independent of clock order. A fork,
// missing ancestor or cycle fails closed, including after a process restart.
fn latest_consent(
    records: Vec<AgentAccessRecord>,
    sandbox: &Sandbox,
) -> Result<Option<AgentAccessRecord>, AgentAccessError> {
    if records.is_empty() {
        return Ok(None);
    }
    let mut by_id = BTreeMap::new();
    let mut parents = BTreeSet::new();
    for record in records {
        if record.requester.owner != sandbox.owner
            || record.requester.sandbox_id != sandbox.id
            || record.approved_services()? != sandbox.credential.effective_reaches()
            || record.progress.grant_generation_floor
                != Some(sandbox.credential.service_access_generation_floor())
            || record
                .progress
                .renewed_from
                .as_ref()
                .is_some_and(|id| !parents.insert(id.clone()))
            || by_id.insert(record.id.clone(), record).is_some()
        {
            return Err(AgentAccessError::Invalid);
        }
    }
    let leaves: Vec<_> = by_id.keys().filter(|id| !parents.contains(*id)).collect();
    let [leaf] = leaves.as_slice() else {
        return Err(AgentAccessError::Invalid);
    };
    let latest = by_id.get(*leaf).ok_or(AgentAccessError::Invalid)?;
    let mut current = latest;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(&current.id) {
            return Err(AgentAccessError::Invalid);
        }
        let Some(parent) = &current.progress.renewed_from else {
            break;
        };
        current = by_id.get(parent).ok_or(AgentAccessError::Invalid)?;
    }
    if seen.len() != by_id.len() {
        return Err(AgentAccessError::Invalid);
    }
    Ok(Some(latest.clone()))
}

impl MongoSandboxStore {
    pub(crate) async fn credential_scope_authorization(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<AgentAuthorizationRevision>, AgentAccessError> {
        if sandbox.credential.service_access_generation_floor() == 0 {
            return Ok(None);
        }
        let rows: Vec<_> = self
            .agent_access_requests
            .find(scope_filter(sandbox)?)
            .await
            .map_err(|_| AgentAccessError::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| AgentAccessError::Unavailable)?;
        let records = rows.iter().map(read).collect::<Result<Vec<_>, _>>()?;
        let Some(record) = latest_consent(records, sandbox)? else {
            return Ok(None); // Historical grants retain owner login custody.
        };
        let Some(AgentAccessDecision::Approved { authorization, .. }) = record.decision else {
            return Err(AgentAccessError::Forbidden);
        };
        Ok(Some(authorization))
    }
}

pub(super) async fn renew_current_scope(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    next: &mut AgentAccessRecord,
    session: &mut ClientSession,
) -> TxResult<()> {
    let Some(scope) = &sandbox.credential.service_scope else {
        return Ok(());
    };
    if next.approved_services()? != scope.services
        || !matches!(&next.decision, Some(AgentAccessDecision::Approved { authorization, .. }) if authorization.authority_id.starts_with("owner-api:"))
        || !(sandbox.needs_service_access_credential()
            || sandbox.status == SandboxStatus::CredentialRotating
            || sandbox.credential.pending.is_some()
            || !sandbox.credential.revoke_debt.is_empty())
    {
        return Ok(());
    }
    let mut cursor = store
        .agent_access_requests
        .find(scope_filter(sandbox)?)
        .session(&mut *session)
        .await?;
    let mut records = Vec::new();
    while let Some(value) = cursor.next(&mut *session).await {
        records.push(read(&value?)?);
    }
    let Some(previous) = latest_consent(records, sandbox)? else {
        return Ok(());
    };
    if !matches!(&previous.decision, Some(AgentAccessDecision::Approved { authorization, .. }) if authorization.authority_id.starts_with("owner-api:"))
    {
        return Ok(());
    }
    if !previous.phase.terminal()
        || !matches!(
            sandbox.status,
            SandboxStatus::Active | SandboxStatus::CredentialRotating
        )
        || sandbox.lifecycle.operation.is_some()
        || previous.requester.creation != next.requester.creation
        || previous.requester.conversation != next.requester.conversation
        || next.requester.runtime.provision_generation.is_none()
    {
        return Err(AgentAccessError::Conflict.into());
    }
    // The caller has independently revalidated owner identity, exact services
    // and backing routes. Reauthorize pending renewal/revocation without a new
    // scope revision (which requires the previous rotation to have settled).
    next.phase = AgentAccessPhase::Updating;
    next.expected_scope_revision = Some(
        scope
            .revision
            .checked_sub(1)
            .ok_or(AgentAccessError::Invalid)?,
    );
    next.progress = AgentAccessProgress {
        renewed_from: Some(previous.id),
        grant_generation_floor: Some(scope.credential_generation_floor),
        hot_update: Some(AgentAccessHotUpdate {
            runtime: next.requester.runtime.clone(),
            lifecycle_next_authority: sandbox.lifecycle.next_authority,
        }),
        ..Default::default()
    };
    Ok(())
}

#[cfg(test)]
#[path = "authority_tests.rs"]
mod tests;
