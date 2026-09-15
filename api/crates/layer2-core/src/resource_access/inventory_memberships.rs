//! Current organization evidence shared by resource inventory readers.
use super::*;
use crate::SubjectId;
use futures_util::{stream, StreamExt, TryStreamExt};

pub(crate) struct InventoryMemberships {
    pub context: CallerContext,
    organizations: Vec<(SubjectId, CallerContext)>,
}
impl InventoryMemberships {
    pub async fn load(
        access: &ResourceAccessService,
        context: &CallerContext,
        credential: Option<&str>,
        organization: Option<&SubjectId>,
    ) -> Result<Self, ResourceAccessError> {
        let ids = if let Some(id) = organization {
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
                    if organization.is_none() =>
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
            context: current,
            organizations,
        };
        value.require(access)?;
        Ok(value)
    }
    pub fn ids(&self) -> Vec<SubjectId> {
        self.organizations
            .iter()
            .map(|(id, _)| id.clone())
            .collect()
    }
    pub fn require(&self, access: &ResourceAccessService) -> Result<(), ResourceAccessError> {
        for (id, context) in &self.organizations {
            access.require_inventory_membership(context, id)?;
        }
        Ok(())
    }
}
