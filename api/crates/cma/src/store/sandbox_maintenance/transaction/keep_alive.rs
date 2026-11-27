use super::*;

pub(super) async fn keep_alive_and_fence_policy(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    auto_pause_enabled: bool,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    if sandbox.profile.always_on == Some(true) {
        return Ok(true);
    }
    if auto_pause_enabled {
        // Global policy ignores historical Profile availability. Its settings
        // revision and the explicit preference on the Sandbox are fenced by this
        // transaction, including when a retained Profile has been removed.
        return Ok(false);
    }
    if store
        .bindings
        .keep_alive(sandbox, session)
        .await
        .map_err(ApplyError::Domain)?
        == Some(true)
    {
        return Ok(true);
    }
    let Some(document) = store
        .sandbox_profiles
        .find_one(doc! { "_id": sandbox.profile.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
    else {
        return super::super::retained_availability::keep_alive(store, sandbox, session).await;
    };
    let profile = crate::store::sandbox_profiles::read_profile(&document)
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    if profile.availability.always_keep_alive {
        return Ok(true);
    }
    fence_policy(store, sandbox, session).await
}

async fn fence_policy(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    // An actual write conflicts with any concurrent policy edit. Merely reading
    // the Profile under snapshot isolation permits an obsolete pause to commit.
    let fenced = store
        .sandbox_profiles
        .update_one(
            doc! { "_id": sandbox.profile.id.as_str() },
            doc! { "$inc": { "inactivity_fence": 1_i64 } },
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if fenced.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(false)
}
