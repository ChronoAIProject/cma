//! Fresh membership checks for one subscriber. The caller's original resource
//! fence is checked against the later cohort snapshot before any data release.
use super::*;
use crate::{Authorization, SubjectId};
use std::{collections::BTreeSet, time::SystemTime};

impl ResourceAccessService {
    pub(crate) async fn observation_memberships(
        &self,
        held: &AuthorizedResource,
        target: &ResourceTarget,
        context: &CallerContext,
        credential: Option<&str>,
    ) -> Result<(), ResourceAccessError> {
        let capability = ResourceCapability::ViewHistory;
        held.decision.require(capability)?;
        if held.binding.is_some() {
            return Ok(());
        }
        if !context.validate_principal() {
            return Err(AccessError::NotFound.into());
        }
        let scope = held.snapshot.scope(target)?;
        let mut current = context.clone();
        current.memberships.clear();
        if AccessDecision::evaluate(
            &current,
            &scope,
            &held.snapshot.shares,
            AccessPurpose::Read,
            SystemTime::now(),
        )?
        .allows(capability)
        {
            return Ok(());
        }
        let organizations: BTreeSet<_> = held
            .snapshot
            .shares
            .iter()
            .filter(|share| {
                share
                    .link
                    .as_ref()
                    .is_none_or(|link| link.accepted_subjects.contains(&context.caller.subject))
            })
            .filter_map(|share| share.organization_recipient().cloned())
            .chain(match &scope.owner {
                ResourceOwner::Organization { subject } => Some(subject.clone()),
                _ => None,
            })
            .collect();
        let credential = credential.ok_or(MembershipError::Unavailable)?;
        let mut unavailable = false;
        for organization in organizations {
            match self
                .organizations
                .organization_context(
                    &context.caller,
                    credential,
                    &SubjectId::new(organization.to_string()),
                )
                .await
            {
                Ok(evidence)
                    if evidence.validate_principal()
                        && evidence.credential == context.credential
                        && evidence.caller.subject == context.caller.subject
                        && evidence.memberships.len() == 1
                        && evidence.memberships[0].organization == organization =>
                {
                    current.caller.authorization =
                        Authorization::new(current.caller.authorization.iter().filter(
                            |capability| evidence.caller.authorization.allows(*capability),
                        ));
                    current.memberships.extend(evidence.memberships);
                }
                Err(MembershipError::NotMember) => {}
                _ => unavailable = true,
            }
        }
        let decision = AccessDecision::evaluate(
            &current,
            &scope,
            &held.snapshot.shares,
            AccessPurpose::Read,
            SystemTime::now(),
        )?;
        if let Err(error) = decision.require(capability) {
            return Err(if unavailable {
                MembershipError::Unavailable.into()
            } else {
                error.into()
            });
        }
        // New membership evidence may narrow this release. It cannot extend the
        // original stream's lifetime or become another subscriber's decision.
        held.decision.require(capability)?;
        Ok(())
    }
}
