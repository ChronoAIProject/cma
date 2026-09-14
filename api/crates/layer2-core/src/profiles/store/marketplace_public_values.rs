use crate::profiles::{MarketplaceError, PublicValueScanner};

use super::*;

pub(super) struct InspectedSandbox {
    profile: SandboxProfile,
    revisions: Vec<SandboxProfileRevision>,
}

impl InspectedSandbox {
    pub(super) fn verify(&self, state: &State) -> Result<(), MarketplaceError> {
        let current = revisions(state, &self.profile.id);
        if state.sandboxes.get(&self.profile.id) != Some(&self.profile) || current != self.revisions
        {
            return Err(MarketplaceError::Unavailable(
                "profile changed during public-value inspection".into(),
            ));
        }
        Ok(())
    }
}

fn revisions(state: &State, id: &SandboxProfileId) -> Vec<SandboxProfileRevision> {
    let mut rows: Vec<_> = state
        .sandbox_revisions
        .iter()
        .filter(|((stored, _), _)| stored == id)
        .map(|(_, revision)| revision.clone())
        .collect();
    rows.sort_by_key(|revision| revision.revision);
    rows
}

pub(super) async fn inspect(
    store: &InMemoryProfileStore,
    owner: &SubjectId,
    locator: &MarketplaceLocator,
    visibility: ProfileVisibility,
    bounds: ProfileCapacityBounds,
    scanner: &dyn PublicValueScanner,
) -> Result<Option<InspectedSandbox>, MarketplaceError> {
    let MarketplaceLocator::Sandbox(id) = locator else {
        return Ok(None);
    };
    if visibility != ProfileVisibility::Public {
        return Ok(None);
    }
    let inspected = {
        let state = store.lock();
        let profile = state
            .sandboxes
            .get(id)
            .filter(|profile| &profile.owner == owner)
            .ok_or(MarketplaceError::NotFound)?
            .clone();
        InspectedSandbox {
            profile,
            revisions: revisions(&state, id),
        }
    };
    for revision in &inspected.revisions {
        revision
            .content
            .validate(bounds)
            .map_err(|error| MarketplaceError::Validation(error.code.into()))?;
        revision
            .content
            .validate_public_values(scanner)
            .await
            .map_err(MarketplaceError::PublicValues)?;
    }
    Ok(Some(inspected))
}
