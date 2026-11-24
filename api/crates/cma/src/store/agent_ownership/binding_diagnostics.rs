//! Read-only bounded membership and current-resource checks for operator observations.
use super::*;
use crate::binding_diagnostics::{MemberPage, MembershipSource};
use layer2_core::{
    binding_diagnostics::{BindingDiagnosticsError, MAX_HOLDS_PER_REPORTER, MAX_REPORTERS},
    SandboxId, SubjectId,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

const QUERY_BUDGET: Duration = Duration::from_secs(1);

impl MongoAgentOwnership {
    async fn binding_diagnostic_members(&self) -> Result<MemberPage, BindingDiagnosticsError> {
        let now = self
            .server_now()
            .await
            .map_err(|_| BindingDiagnosticsError::Unavailable)?;
        let mut cursor = self
            .members
            .find(doc! {"expires_at": {"$gt": mongodb::bson::DateTime::from_millis(now)}})
            .projection(member_projection())
            .hint(mongodb::options::Hint::Name(
                "agent_manager_members_expiry".into(),
            ))
            .sort(doc! {"expires_at":1})
            .limit((MAX_REPORTERS + 1) as i64)
            .batch_size((MAX_REPORTERS + 1) as u32)
            .max_time(QUERY_BUDGET)
            .await
            .map_err(|_| BindingDiagnosticsError::Unavailable)?;
        let mut members = Vec::new();
        let mut truncated = false;
        while let Some(row) = cursor
            .try_next()
            .await
            .map_err(|_| BindingDiagnosticsError::Unavailable)?
        {
            if members.len() == MAX_REPORTERS {
                truncated = true;
                break;
            }
            members.push(member_from(&row).map_err(|_| BindingDiagnosticsError::Unavailable)?);
        }
        Ok(MemberPage { members, truncated })
    }

    async fn check_binding_diagnostic_members(
        &self,
        sender: &str,
        recipient: &str,
    ) -> Result<(), BindingDiagnosticsError> {
        for value in [sender, recipient] {
            if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
                return Err(BindingDiagnosticsError::Unavailable);
            }
        }
        let ids: BTreeSet<_> = [sender, recipient].into_iter().collect();
        let expected = ids.len();
        let rows: Vec<Document> = self
            .members
            .find(doc! {
                "_id": {"$in": ids.into_iter().collect::<Vec<_>>()},
                "protocol": i64::from(PROTOCOL_VERSION), "draining": false,
                "$expr": {"$gt": ["$expires_at", "$$NOW"]},
            })
            .projection(member_projection())
            .limit(3)
            .batch_size(3)
            .max_time(QUERY_BUDGET)
            .await
            .map_err(|_| BindingDiagnosticsError::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| BindingDiagnosticsError::Unavailable)?;
        if rows.len() != expected || rows.iter().any(|row| member_from(row).is_err()) {
            return Err(BindingDiagnosticsError::Unavailable);
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl MembershipSource for super::super::MongoSandboxStore {
    async fn list(&self) -> Result<MemberPage, BindingDiagnosticsError> {
        self.agent_ownership
            .as_ref()
            .ok_or(BindingDiagnosticsError::Unavailable)?
            .binding_diagnostic_members()
            .await
    }

    async fn check(&self, sender: &str, recipient: &str) -> Result<(), BindingDiagnosticsError> {
        self.agent_ownership
            .as_ref()
            .ok_or(BindingDiagnosticsError::Unavailable)?
            .check_binding_diagnostic_members(sender, recipient)
            .await
    }

    async fn current_owners(
        &self,
        ids: &[SandboxId],
    ) -> Result<BTreeMap<SandboxId, SubjectId>, BindingDiagnosticsError> {
        current_owners(&self.sandboxes, ids).await
    }
}

async fn current_owners(
    rows: &Collection<Document>,
    ids: &[SandboxId],
) -> Result<BTreeMap<SandboxId, SubjectId>, BindingDiagnosticsError> {
    if ids.len() > MAX_REPORTERS * MAX_HOLDS_PER_REPORTER
        || ids.iter().any(|id| {
            id.as_str().is_empty()
                || id.as_str().len() > 256
                || id.as_str().chars().any(char::is_control)
        })
    {
        return Err(BindingDiagnosticsError::Unavailable);
    }
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let wanted: BTreeSet<_> = ids.iter().cloned().collect();
    let mut cursor = rows.find(doc! {"_id":{"$in":wanted.iter().map(|id| id.as_str()).collect::<Vec<_>>()}})
        .projection(doc! {"_id":bounded_string("$_id",257),"owner":bounded_string("$owner",513),"document_schema_version":integer("$document_schema_version")})
        .limit((wanted.len() + 1) as i64).batch_size(64).max_time(QUERY_BUDGET)
        .await.map_err(|_| BindingDiagnosticsError::Unavailable)?;
    let mut owners = BTreeMap::new();
    while let Some(row) = cursor
        .try_next()
        .await
        .map_err(|_| BindingDiagnosticsError::Unavailable)?
    {
        super::super::work_schema::require(&row)
            .map_err(|_| BindingDiagnosticsError::Unavailable)?;
        let id = row
            .get_str("_id")
            .map_err(|_| BindingDiagnosticsError::Unavailable)?;
        let owner = row
            .get_str("owner")
            .map_err(|_| BindingDiagnosticsError::Unavailable)?;
        if id.is_empty()
            || id.len() > 256
            || owner.is_empty()
            || owner.len() > 512
            || owner.chars().any(char::is_control)
        {
            return Err(BindingDiagnosticsError::Unavailable);
        }
        let id = SandboxId(id.into());
        if !wanted.contains(&id)
            || owners.insert(id, SubjectId(owner.into())).is_some()
            || owners.len() > wanted.len()
        {
            return Err(BindingDiagnosticsError::Unavailable);
        }
    }
    Ok(owners)
}

fn member_projection() -> Document {
    doc! {
        "_id":bounded_string("$_id",129),
        "document_schema_version":integer("$document_schema_version"),
        "protocol":integer("$protocol"),
        "relay_address":bounded_string("$relay_address",2049),
        "draining":{"$cond":[{"$eq":[{"$type":"$draining"},"bool"]},"$draining",null]},
        "owned_agents":integer("$owned_agents"),"max_owned_agents":integer("$max_owned_agents"),
        "expires_at":{"$cond":[{"$eq":[{"$type":"$expires_at"},"date"]},"$expires_at",null]},
    }
}

fn integer(path: &str) -> Document {
    doc! {"$cond":[{"$in":[{"$type":path},["int","long"]]},{"$toLong":path},-1_i64]}
}

fn bounded_string(path: &str, max: i32) -> Document {
    doc! {"$cond":[{"$eq":[{"$type":path},"string"]},{"$substrCP":[path,0,max]},null]}
}

#[cfg(test)]
#[path = "binding_diagnostics_tests.rs"]
mod tests;
