//! Canonical erased attribution preserves immutable history links. These
//! comparisons never establish resource authority or permit other differences.
use super::*;
use crate::sandbox::AgentCreationOrigin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RetainedDigest {
    pub original: String,
    pub content: String,
}

fn erased(actor: &ResourceActor) -> bool {
    match actor {
        ResourceActor::Human { subject } | ResourceActor::Trigger { subject, .. } => {
            subject.as_str() == crate::DELETED_ACCOUNT_SUBJECT
        }
        ResourceActor::Api {
            subject,
            delegation_id,
        } => subject.as_str() == crate::DELETED_ACCOUNT_SUBJECT && delegation_id.is_none(),
        ResourceActor::Channel { .. } => false,
    }
}

fn actors_match(left: Option<&ResourceActor>, right: Option<&ResourceActor>) -> bool {
    if left == right {
        return true;
    }
    match (left, right) {
        (Some(left), Some(right)) if erased(left) || erased(right) => match (left, right) {
            (ResourceActor::Human { .. }, ResourceActor::Human { .. })
            | (ResourceActor::Api { .. }, ResourceActor::Api { .. }) => true,
            (
                ResourceActor::Trigger {
                    launch_id: left, ..
                },
                ResourceActor::Trigger {
                    launch_id: right, ..
                },
            ) => left == right,
            _ => false,
        },
        _ => false,
    }
}

/// Accept the temporary difference between two retained copies of the same
/// creation facts while canonical attribution is erased in separate batches.
pub fn same_creation_identity(
    left: &AgentCreationProvenance,
    right: &AgentCreationProvenance,
) -> bool {
    if !actors_match(left.created_by.as_ref(), right.created_by.as_ref()) {
        return false;
    }
    let mut left = left.clone();
    let mut right = right.clone();
    left.created_by = None;
    right.created_by = None;
    if let (
        AgentCreationOrigin::Trigger {
            launching_actor: left_actor,
            ..
        },
        AgentCreationOrigin::Trigger {
            launching_actor: right_actor,
            ..
        },
    ) = (&mut left.creation_origin, &mut right.creation_origin)
    {
        if !actors_match(left_actor.as_ref(), right_actor.as_ref()) {
            return false;
        }
        *left_actor = None;
        *right_actor = None;
    }
    left == right
}

/// State revisions, bindings, times and all non-attribution facts remain exact.
pub fn same_usage_state(left: &AgentChannelUsageState, right: &AgentChannelUsageState) -> bool {
    if !same_creation_identity(&left.creation, &right.creation) {
        return false;
    }
    let mut left = left.clone();
    left.creation = right.creation.clone();
    left == *right
}

impl ChannelUsageEvent {
    /// Storage readers may retain the original event digest only for a valid
    /// canonical erased record whose new content digest is independently stored.
    /// Original command retries still compare the changed input fingerprint.
    pub fn retain_erased_identity_digest(
        &mut self,
        original: &str,
        content: &str,
    ) -> Result<(), SandboxStoreError> {
        self.retained_digest = None;
        self.validate()?;
        let creation = &self.before.creation;
        let has_erased = erased(&self.actor)
            || creation.created_by.as_ref().is_some_and(erased)
            || matches!(&creation.creation_origin, AgentCreationOrigin::Trigger { launching_actor: Some(actor), .. } if erased(actor));
        if !has_erased || !digest(original) || !digest(content) || hash(self) != content {
            return Err(corrupt());
        }
        self.retained_digest = Some(RetainedDigest {
            original: original.into(),
            content: content.into(),
        });
        Ok(())
    }

    pub(super) fn validate_retained_digest(&self) -> Result<(), SandboxStoreError> {
        if self
            .retained_digest
            .as_ref()
            .is_some_and(|retained| !digest(&retained.original) || retained.content != hash(self))
        {
            return Err(corrupt());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SubjectId, DELETED_ACCOUNT_SUBJECT};

    #[test]
    fn shared_deleted_subject_matches_canonical_retained_attribution() {
        let original = ResourceActor::Human {
            subject: SubjectId::new("original-person"),
        };
        let deleted = ResourceActor::Human {
            subject: SubjectId::new(DELETED_ACCOUNT_SUBJECT),
        };
        assert!(erased(&deleted));
        assert!(actors_match(Some(&original), Some(&deleted)));
        let api = ResourceActor::Api {
            subject: SubjectId::new(DELETED_ACCOUNT_SUBJECT),
            delegation_id: None,
        };
        assert!(erased(&api));
    }

    #[test]
    fn deleted_subject_cannot_preserve_delegation_or_change_actor_kind() {
        let api = ResourceActor::Api {
            subject: SubjectId::new(DELETED_ACCOUNT_SUBJECT),
            delegation_id: Some("retained-grant".into()),
        };
        assert!(!erased(&api));
        assert!(!erased(&ResourceActor::Human {
            subject: SubjectId::new("deleted-account-other"),
        }));
        assert!(!actors_match(
            Some(&ResourceActor::Human {
                subject: SubjectId::new(DELETED_ACCOUNT_SUBJECT),
            }),
            Some(&api),
        ));
    }
}
