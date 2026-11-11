//! Authoring checks use persisted authority; source selection cannot claim an App.
use super::management::{ManagementError, ManagementState};
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    ValidationError,
};

pub(super) async fn validate(
    state: &ManagementState,
    current: Option<&ChannelDefinition>,
    next: &ChannelDefinition,
) -> Result<(), ManagementError> {
    for source in &next.sources {
        let Source::Github {
            app_ref,
            installation_ids,
            repository_ids,
            ..
        } = source
        else {
            continue;
        };
        if current.is_some_and(|old| old.sources.contains(source)) {
            continue;
        }
        let gateway = state
            .github
            .get()
            .ok_or_else(|| invalid("github_source_unavailable"))?;
        let app = gateway
            .read_app(&next.owner.subject, app_ref)
            .await
            .map_err(map_error)?;
        if app.status != "verified" || !app.webhook_credential.present {
            return Err(invalid("github_integration_not_ready"));
        }
        let mut approved = std::collections::BTreeSet::new();
        for id in installation_ids {
            let binding = gateway
                .installation_binding(&next.owner.subject, &app, *id)
                .await
                .map_err(map_error)?;
            if binding.status != "active" {
                return Err(invalid("github_installation_not_approved"));
            }
            approved.extend(
                binding
                    .repository_ids
                    .iter()
                    .copied()
                    .filter(|id| !binding.excluded_repository_ids.contains(id)),
            );
        }
        if repository_ids.iter().any(|id| !approved.contains(id)) {
            return Err(invalid("github_repository_not_approved"));
        }
    }
    Ok(())
}
fn invalid(reason: &str) -> ManagementError {
    ManagementError::Invalid(ValidationError::new("/sources", reason))
}
fn map_error(error: crate::github::Error) -> ManagementError {
    match error {
        crate::github::Error::NotFound => ManagementError::NotFound,
        _ => invalid("github_source_unavailable"),
    }
}
