use super::*;
use crate::resource_access::{AccessDecision, MembershipError, ResourceOwner};
use std::time::SystemTime;

pub(super) struct InventoryAuthority {
    pub caller: Caller,
    // None selects explicit incoming shares. Ownership itself has no shared kind.
    pub scope: Option<ResourceOwner>,
    pub context: Option<CallerContext>,
    organizations: Vec<(SubjectId, CallerContext)>,
}

impl InventoryAuthority {
    pub fn personal(caller: &Caller) -> Self {
        Self {
            caller: caller.clone(),
            scope: Some(ResourceOwner::Person {
                subject: caller.subject.clone(),
            }),
            context: None,
            organizations: Vec::new(),
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
        let ids = if let Some(id) = &query.organization {
            vec![id.clone()]
        } else {
            let choices = access.organization_choices(context, credential).await?;
            // Bound discovery fanout and memory independently of Agent page size.
            if choices.len() > 128 {
                return Err(MembershipError::Unavailable.into());
            }
            choices.into_iter().map(|choice| choice.id).collect()
        };
        if ids.iter().any(|id| {
            id.as_str().is_empty() || id.as_str().len() > 128 || id.as_str().trim() != id.as_str()
        }) {
            return Err(MembershipError::InvalidEvidence.into());
        }
        let organizations: Vec<_> = stream::iter(ids.into_iter().map(|id| async move {
            match access.inventory_context(context, credential, &id).await {
                Ok(current) => Ok(Some((id, current))),
                // Choices can lag removal. Only an actual provider refusal can
                // remove a choice; an outage must not look like an empty inventory.
                Err(ResourceAccessError::Membership(MembershipError::NotMember))
                    if query.shared =>
                {
                    Ok(None)
                }
                Err(error) => Err(error),
            }
        }))
        .buffered(8)
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .flatten()
        .collect();
        let mut current = context.clone();
        current.memberships.clear();
        current.caller.authorization =
            crate::Authorization::new(context.caller.authorization.iter().filter(|capability| {
                organizations
                    .iter()
                    .all(|(_, evidence)| evidence.caller.authorization.allows(*capability))
            }));
        for (_, evidence) in &organizations {
            current
                .memberships
                .extend(evidence.memberships.iter().cloned());
        }
        let value = Self {
            caller: context.caller.clone(),
            scope: query
                .organization
                .as_ref()
                .map(|id| ResourceOwner::Organization {
                    subject: id.clone(),
                }),
            context: Some(current),
            organizations,
        };
        value.require_membership(access)?;
        Ok(value)
    }

    pub fn organization_ids(&self) -> Vec<SubjectId> {
        self.organizations
            .iter()
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn require_membership(
        &self,
        access: &ResourceAccessService,
    ) -> Result<(), ResourceAccessError> {
        for (id, context) in &self.organizations {
            access.require_inventory_membership(context, id)?;
        }
        Ok(())
    }
}

pub(super) fn authorize_candidates(
    authority: &InventoryAuthority,
    query: &AgentListQuery,
    scan: &PageRequest,
    snapshots: &[AgentProvenanceSnapshot],
) -> Result<Vec<AuthorizedResource>, ResourceAccessError> {
    let mut previous = scan.cursor.as_deref();
    let mut result = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
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
            snapshot: snapshot.resource.clone(),
            decision,
        });
    }
    Ok(result)
}
