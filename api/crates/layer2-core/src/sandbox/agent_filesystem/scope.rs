use super::AgentFileError;
use crate::resource_access::{
    AccessError, AuthorizedResource, ResourceAccessError, ResourceCapability,
};
use crate::AgentId;

pub(super) fn require_home_mutation(
    authorized: &AuthorizedResource,
    subject: &crate::SubjectId,
) -> Result<(), AgentFileError> {
    let owner = authorized
        .decision
        .allows(ResourceCapability::ManageSharing);
    let linked_write = authorized.snapshot.shares.iter().any(|share| {
        share.mode == crate::resource_access::ShareMode::ViewWrite
            && share.revoked_at.is_none()
            && authorized
                .decision
                .share_revisions
                .contains(&(share.id.clone(), share.revision))
            && share
                .link
                .as_ref()
                .is_some_and(|link| link.accepted_subjects.contains(subject))
    });
    if owner || linked_write {
        Ok(())
    } else {
        Err(ResourceAccessError::Access(AccessError::Forbidden).into())
    }
}

pub(super) fn public_path(
    agent_id: &AgentId,
    path: &str,
    remove: bool,
) -> Result<(), AgentFileError> {
    if !layer3_adapter::valid_agent_file_path(path)
        || [".cma", "cma/.cma"].iter().any(|protected| {
            path == *protected
                || path.starts_with(&format!("{protected}/"))
                || (remove && (path.is_empty() || protected.starts_with(&format!("{path}/"))))
        })
    {
        return Err(AgentFileError::Invalid);
    }
    if (remove && path == "attachments")
        || path
            .strip_prefix("attachments/")
            .is_some_and(|suffix| suffix.split('/').next() != Some(agent_id.0.as_str()))
    {
        return Err(AgentFileError::Invalid);
    }
    if path.split('/').any(|component| component.len() > 255) {
        return Err(AgentFileError::NameTooLong);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_path_component_bounds_count_utf8_bytes() {
        let agent = AgentId("test-agent".into());
        for name in [
            "a".repeat(255),
            format!("{}.txt", "a".repeat(251)),
            "界".repeat(85),
            "普通 # note.txt".into(),
        ] {
            assert!(public_path(&agent, &format!("cma/{name}"), false).is_ok());
        }
        for name in [
            "a".repeat(256),
            format!("{}.txt", "a".repeat(252)),
            "界".repeat(86),
        ] {
            assert!(matches!(
                public_path(&agent, &format!("cma/{name}"), false),
                Err(AgentFileError::NameTooLong)
            ));
        }
        assert!(matches!(
            public_path(&agent, "../outside", false),
            Err(AgentFileError::Invalid)
        ));
    }
}
