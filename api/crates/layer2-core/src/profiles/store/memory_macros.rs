// Shared optimistic updates for in-memory profile aggregates.
macro_rules! save_draft {
    ($state:expr, $collection:ident, $id:expr, $expected:expr, $profile:expr) => {{
        let stored = $state
            .$collection
            .get(&$id)
            .ok_or(ProfileStoreError::NotFound)?;
        let next_version = $expected
            .checked_add(1)
            .ok_or(ProfileStoreError::Conflict)?;
        if stored.draft_version != $expected
            || $profile.draft_version != next_version
            || stored.internal != $profile.internal
            || stored.owner != $profile.owner
            || stored.visibility != $profile.visibility
            || stored.forked_from != $profile.forked_from
            || stored.uses_count != $profile.uses_count
            || stored.created_at != $profile.created_at
            || stored.retired_at != $profile.retired_at
        {
            return Err(ProfileStoreError::Conflict);
        }
        $profile.validate()?;
        $state.$collection.insert($id, $profile);
        Ok(())
    }};
}

macro_rules! retire_profile {
    ($state:expr, $collection:ident, $id:expr, $owner:expr, $at:expr) => {{
        let profile = $state
            .$collection
            .get_mut($id)
            .filter(|profile| !profile.internal && &profile.owner == $owner)
            .ok_or(ProfileStoreError::NotFound)?;
        profile.retired_at = Some($at.clone());
        profile.updated_at = $at;
        Ok(())
    }};
}
