use super::*;
use crate::resource_access::{AccessDecision, ResourceOwner};
use std::time::SystemTime;

pub(super) struct InventoryAuthority {
    pub caller: Caller,
    // None selects explicit incoming shares. Ownership itself has no shared kind.
    pub scope: Option<ResourceOwner>,
    pub context: Option<CallerContext>,
    memberships: Option<crate::resource_access::InventoryMemberships>,
}

impl InventoryAuthority {
    pub fn personal(caller: &Caller) -> Self {
        Self {
            caller: caller.clone(),
            scope: Some(ResourceOwner::Person {
                subject: caller.subject.clone(),
            }),
            context: None,
            memberships: None,
        }
    }

    pub async fn load(
        access: &ResourceAccessService,
        context: &CallerContext,
        credential: Option<&str>,
        query: &AgentProvenanceInventoryQuery,
    ) -> Result<Self, ResourceAccessError> {
        if !query.shared && query.organization.is_none() {
            return Ok(Self::personal(&context.caller));
        }
        let memberships = crate::resource_access::InventoryMemberships::load(
            access,
            context,
            credential,
            query.organization.as_ref(),
        )
        .await?;
        let value = Self {
            caller: context.caller.clone(),
            scope: query
                .organization
                .as_ref()
                .map(|id| ResourceOwner::Organization {
                    subject: id.clone(),
                }),
            context: Some(memberships.context.clone()),
            memberships: Some(memberships),
        };
        value.require_membership(access)?;
        Ok(value)
    }

    pub fn organization_ids(&self) -> Vec<SubjectId> {
        self.memberships
            .as_ref()
            .map_or_else(Vec::new, |value| value.ids())
    }

    pub fn require_membership(
        &self,
        access: &ResourceAccessService,
    ) -> Result<(), ResourceAccessError> {
        if let Some(memberships) = &self.memberships {
            memberships.require(access)?;
        }
        Ok(())
    }
}

pub(super) fn authorize_candidates(
    authority: &InventoryAuthority,
    query: &AgentListQuery,
    scan: &PageRequest,
    snapshots: &[AgentChannelUsageSnapshot],
) -> Result<Vec<AuthorizedResource>, ResourceAccessError> {
    let mut previous = scan.cursor.as_deref();
    let mut result = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let snapshot = &snapshot.provenance;
        let agent = snapshot
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?;
        if !query.matches(agent) || previous.is_some_and(|cursor| agent.id.as_str() <= cursor) {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        previous = Some(agent.id.as_str());
        let scope = snapshot.resource.scope(&ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        })?;
        let decision = match &authority.context {
            Some(context) => {
                match &authority.scope {
                    Some(ResourceOwner::Organization { subject }) => {
                        snapshot.resource.organization_agent(subject)?;
                    }
                    None => {
                        snapshot.resource.shared_agent(
                            &authority.caller.subject,
                            &authority.organization_ids(),
                        )?;
                    }
                    _ => return Err(ResourceAccessStoreError::Corrupt.into()),
                }
                AccessDecision::evaluate(
                    context,
                    &scope,
                    &snapshot.resource.shares,
                    AccessPurpose::Read,
                    SystemTime::now(),
                )?
            }
            None => {
                if Some(&scope.owner) != authority.scope.as_ref() {
                    return Err(ResourceAccessStoreError::Corrupt.into());
                }
                AccessDecision::personal(&authority.caller, &authority.caller.subject)
            }
        };
        decision.require(READ_CAPABILITY)?;
        result.push(AuthorizedResource {
            binding: None,
            caller: authority.caller.clone(),
            snapshot: snapshot.resource.clone(),
            decision,
        });
    }
    Ok(result)
}
