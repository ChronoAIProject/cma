use std::{
    collections::BTreeSet,
    time::{Duration, SystemTime},
};

use super::*;
use crate::sandbox::store::{Page, PageRequest};
use crate::sandbox::{Agent, AgentListQuery};
use crate::{Authorization, Capability, SubjectId};
use futures_util::{stream, StreamExt, TryStreamExt};

impl ResourceAccessSnapshot {
    /// Aggregate discovery requires a covering explicit incoming grant even if
    /// some independent authority would allow reading the same resource.
    pub fn shared_agent(
        &self,
        recipient: &SubjectId,
        organizations: &[SubjectId],
    ) -> Result<&Agent, ResourceAccessStoreError> {
        let agent = self
            .lineage
            .agent
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?;
        let scope = self.scope(&ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        })?;
        if matches!(&scope.owner, ResourceOwner::Person { subject } if subject == recipient)
            || !self.shares.iter().any(|share| {
                share.incoming_for(recipient, organizations)
                    && share.revoked_at.is_none()
                    && share.selector.covers(&scope)
            })
        {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(agent)
    }

    /// Validate an inventory candidate against authoritative ancestry and current
    /// ownership/grants. A raw owner string never establishes organization scope.
    pub fn organization_agent(
        &self,
        organization: &SubjectId,
    ) -> Result<&Agent, ResourceAccessStoreError> {
        let agent = self
            .lineage
            .agent
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?;
        let scope = self.scope(&ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        })?;
        let owned = matches!(&scope.owner, ResourceOwner::Organization {subject} if subject == organization);
        if !owned
            && !self.shares.iter().any(|share| {
                share.link.is_none()
                    && &share.target_org_id == organization
                    && share.revoked_at.is_none()
                    && share.selector.covers(&scope)
            })
        {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(agent)
    }
}

impl ResourceAccessService {
    /// An explicit organization scope preserves personal inventory availability
    /// during provider outages. Organization choices are never accepted as proof.
    pub async fn organization_agent_inventory(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        organization: &SubjectId,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Page<AuthorizedResource>, ResourceAccessError> {
        if !context.validate_principal() {
            return Err(AccessError::NotFound.into());
        }
        if !context
            .caller
            .authorization
            .allows(Capability::UseOwnSandboxes)
        {
            return Err(AccessError::Forbidden.into());
        }
        PageRequest::new(page.limit, page.cursor.clone())
            .map_err(|_| ResourceAccessStoreError::Invalid)?;
        if organization.as_str().trim().is_empty()
            || organization.as_str().len() > 128
            || organization.as_str().trim() != organization.as_str()
        {
            return Err(ResourceAccessStoreError::Invalid.into());
        }
        let current = self
            .inventory_context(context, credential, organization)
            .await?;
        // Even a boolean filter result can reveal the parent of a narrow share.
        let mut parents = Vec::new();
        if let Some(id) = &query.sandbox_id {
            parents.push(ResourceTarget::Sandbox {
                sandbox_id: id.clone(),
            });
        }
        if let Some(id) = &query.workspace_id {
            parents.push(ResourceTarget::Workspace {
                workspace_id: id.clone(),
            });
        }
        let mut parent_decisions = Vec::new();
        for parent in parents {
            let authorized = self
                .authorize(
                    context,
                    credential,
                    &parent,
                    ResourceCapability::ViewDetails,
                    AccessPurpose::Read,
                )
                .await?;
            parent_decisions.push((parent, authorized));
        }
        let snapshots = tokio::time::timeout(
            Duration::from_secs(15),
            self.store.organization_agents(organization, query, page),
        )
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)??;
        if snapshots.len() > usize::from(page.limit) + 1 {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        let mut previous = page.cursor.clone();
        let mut authorized = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            let agent = snapshot.organization_agent(organization)?;
            if !query.matches(agent)
                || previous
                    .as_deref()
                    .is_some_and(|id| id >= agent.id.as_str())
            {
                return Err(ResourceAccessStoreError::Corrupt.into());
            }
            previous = Some(agent.id.to_string());
            let scope = snapshot.scope(&ResourceTarget::Agent {
                agent_id: agent.id.clone(),
            })?;
            let decision = AccessDecision::evaluate(
                &current,
                &scope,
                &snapshot.shares,
                AccessPurpose::Read,
                SystemTime::now(),
            )?;
            decision.require(ResourceCapability::ViewDetails)?;
            authorized.push(AuthorizedResource {
                snapshot,
                decision,
                binding: None,
            });
        }
        // Validate the hidden lookahead as well: the presence of `next` must not
        // disclose an Agent whose grant has just disappeared.
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut checks = Vec::with_capacity(authorized.len());
            for value in &authorized {
                checks.push(self.validate_inventory_item(value, organization));
            }
            stream::iter(checks)
                .buffer_unordered(8)
                .try_collect::<Vec<_>>()
                .await?;
            for (target, value) in &parent_decisions {
                self.validate_read_result(value, target, ResourceCapability::ViewDetails)
                    .await?;
            }
            Ok::<_, ResourceAccessError>(())
        })
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)??;
        // Empty inventory also needs current membership; it must not return a
        // misleading successful scope observation after its evidence expires.
        self.require_inventory_membership(&current, organization)?;
        let has_more = authorized.len() > usize::from(page.limit);
        if has_more {
            authorized.pop();
        }
        let next = if has_more {
            authorized.last().map(|value| {
                value
                    .snapshot
                    .lineage
                    .agent
                    .as_ref()
                    .expect("validated Agent inventory")
                    .id
                    .to_string()
            })
        } else {
            None
        };
        Ok(Page {
            items: authorized,
            next,
        })
    }

    async fn validate_inventory_item(
        &self,
        value: &AuthorizedResource,
        organization: &SubjectId,
    ) -> Result<(), ResourceAccessError> {
        let agent = value.snapshot.organization_agent(organization)?;
        self.validate_read_result(
            value,
            &ResourceTarget::Agent {
                agent_id: agent.id.clone(),
            },
            ResourceCapability::ViewDetails,
        )
        .await
    }

    pub(crate) async fn inventory_context(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        organization: &SubjectId,
    ) -> Result<CallerContext, ResourceAccessError> {
        let credential = credential.ok_or(MembershipError::Unavailable)?;
        let evidence = tokio::time::timeout(
            Duration::from_secs(15),
            self.organizations
                .organization_context(&context.caller, credential, organization),
        )
        .await
        .map_err(|_| MembershipError::Unavailable)??;
        if !evidence.validate_principal()
            || evidence.credential != context.credential
            || evidence.caller.subject != context.caller.subject
            || evidence.memberships.len() != 1
            || evidence.memberships[0].organization != *organization
        {
            return Err(MembershipError::InvalidEvidence.into());
        }
        let mut current = context.clone();
        current.caller.authorization = Authorization::new(
            context
                .caller
                .authorization
                .iter()
                .filter(|capability| evidence.caller.authorization.allows(*capability)),
        );
        current.memberships = evidence.memberships;
        self.require_inventory_membership(&current, organization)?;
        Ok(current)
    }

    pub(crate) fn require_inventory_membership(
        &self,
        context: &CallerContext,
        organization: &SubjectId,
    ) -> Result<(), ResourceAccessError> {
        if !context
            .caller
            .authorization
            .allows(Capability::UseOwnSandboxes)
        {
            return Err(AccessError::Forbidden.into());
        }
        if context.memberships.len() != 1
            || context.memberships[0].organization != *organization
            || !context.memberships[0].is_current(&context.credential, SystemTime::now())
        {
            return Err(AccessError::RevalidationRequired.into());
        }
        Ok(())
    }

    /// Presentation choices only. Each subsequent resource request obtains its
    /// own current organization evidence with the actual requesting credential.
    pub async fn organization_choices(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
    ) -> Result<Vec<OrganizationChoice>, ResourceAccessError> {
        if !context.validate_principal() {
            return Err(AccessError::NotFound.into());
        }
        if !context
            .caller
            .authorization
            .allows(Capability::UseOwnSandboxes)
        {
            return Err(AccessError::Forbidden.into());
        }
        let credential = credential.ok_or(MembershipError::Unavailable)?;
        let mut choices = tokio::time::timeout(
            Duration::from_secs(15),
            self.organizations
                .organization_choices(&context.caller, credential),
        )
        .await
        .map_err(|_| MembershipError::Unavailable)??;
        let mut ids = BTreeSet::new();
        if choices
            .iter()
            .any(|choice| choice.id.as_str().trim().is_empty() || !ids.insert(choice.id.as_str()))
        {
            return Err(MembershipError::InvalidEvidence.into());
        }
        choices.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        Ok(choices)
    }
}
