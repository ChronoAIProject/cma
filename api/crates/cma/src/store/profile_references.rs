//! Transactional reads of protected exact-pin evidence; admission policy lives in Core.

use std::collections::BTreeSet;

use layer2_core::profiles::references::{
    reference_is_readable, verify_agent_revision, verify_workspace_revision, InheritedReferences,
};
use layer2_core::profiles::{ProfileError, ProfileStoreError, SandboxProfile, WorkspaceProfile};
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use super::profiles::PublishError;
use super::sandbox_mongo::MongoProfileStore;

pub(super) async fn workspace_references(
    store: &MongoProfileStore,
    parent: &WorkspaceProfile,
    session: &mut ClientSession,
) -> Result<(), PublishError> {
    let database = store
        .client
        .database(&store.workspace_profiles.namespace().db);
    let mut inherited = InheritedReferences::default();
    if let Some(source) = &parent.forked_from {
        let document = find(
            &store.workspace_revisions,
            doc! {
                "profile_id": source.id.as_str(), "revision": i64::from(source.revision),
            },
            session,
        )
        .await?;
        let revision =
            super::workspace_profiles::read_revision(&document).map_err(PublishError::Domain)?;
        inherited
            .inherit_workspace(parent, &revision)
            .map_err(PublishError::Domain)?;
    }
    let filter = doc! { "profile_id": parent.id.as_str() };
    let mut cursor = store
        .workspace_revisions
        .find(filter)
        .session(&mut *session)
        .await
        .map_err(PublishError::Driver)?;
    while let Some(document) = cursor.next(&mut *session).await {
        let revision =
            super::workspace_profiles::read_revision(&document.map_err(PublishError::Driver)?)
                .map_err(PublishError::Domain)?;
        inherited
            .inherit_workspace(parent, &revision)
            .map_err(PublishError::Domain)?;
    }
    for reference in &parent.draft.agent_profiles {
        let document = find(
            &store.agent_profiles,
            doc! { "_id": reference.id.as_str() },
            session,
        )
        .await?;
        let child = super::agent_profiles::read_profile(&document).map_err(PublishError::Domain)?;
        if !reference_is_readable(
            &parent.owner,
            &child.owner,
            child.visibility,
            child.retired_at.is_some(),
            inherited.contains_agent(reference),
        ) {
            return Err(PublishError::Domain(
                ProfileStoreError::ReferenceUnavailable,
            ));
        }
        let document = find(
            &store.agent_revisions,
            doc! {
                "profile_id": reference.id.as_str(), "revision": i64::from(reference.revision),
            },
            session,
        )
        .await?;
        let revision =
            super::agent_profiles::read_revision(&document).map_err(PublishError::Domain)?;
        verify_agent_revision(reference, &revision).map_err(PublishError::Domain)?;
        if !super::profile_pin_admission::agent(
            &database,
            reference.id.as_str(),
            reference.revision,
            Some(&revision.digest),
            session,
        )
        .await
        .map_err(PublishError::Driver)?
        {
            return Err(PublishError::Domain(
                ProfileStoreError::ReferenceUnavailable,
            ));
        }
    }
    Ok(())
}

pub(super) async fn sandbox_references(
    store: &MongoProfileStore,
    parent: &SandboxProfile,
    session: &mut ClientSession,
) -> Result<(), PublishError> {
    let database = store
        .client
        .database(&store.sandbox_profiles.namespace().db);
    let mut inherited = InheritedReferences::default();
    if let Some(source) = &parent.forked_from {
        let document = find(
            &store.sandbox_revisions,
            doc! {
                "profile_id": source.id.as_str(), "revision": i64::from(source.revision),
            },
            session,
        )
        .await?;
        let revision =
            super::sandbox_profiles::read_revision(&document).map_err(PublishError::Domain)?;
        inherited
            .inherit_sandbox(parent, &revision)
            .map_err(PublishError::Domain)?;
    }
    let filter = doc! { "profile_id": parent.id.as_str() };
    let mut cursor = store
        .sandbox_revisions
        .find(filter)
        .session(&mut *session)
        .await
        .map_err(PublishError::Driver)?;
    while let Some(document) = cursor.next(&mut *session).await {
        let revision =
            super::sandbox_profiles::read_revision(&document.map_err(PublishError::Driver)?)
                .map_err(PublishError::Domain)?;
        inherited
            .inherit_sandbox(parent, &revision)
            .map_err(PublishError::Domain)?;
    }
    let mut names = BTreeSet::new();
    for reference in &parent.draft.workspace_profiles {
        let document = find(
            &store.workspace_profiles,
            doc! { "_id": reference.id.as_str() },
            session,
        )
        .await?;
        let child =
            super::workspace_profiles::read_profile(&document).map_err(PublishError::Domain)?;
        if !reference_is_readable(
            &parent.owner,
            &child.owner,
            child.visibility,
            child.retired_at.is_some(),
            inherited.contains_workspace(reference),
        ) {
            return Err(PublishError::Domain(
                ProfileStoreError::ReferenceUnavailable,
            ));
        }
        let document = find(
            &store.workspace_revisions,
            doc! {
                "profile_id": reference.id.as_str(), "revision": i64::from(reference.revision),
            },
            session,
        )
        .await?;
        let revision =
            super::workspace_profiles::read_revision(&document).map_err(PublishError::Domain)?;
        verify_workspace_revision(reference, &revision).map_err(PublishError::Domain)?;
        if !super::profile_pin_admission::workspace(
            &database,
            reference.id.as_str(),
            reference.revision,
            Some(&revision.digest),
            session,
        )
        .await
        .map_err(PublishError::Driver)?
        {
            return Err(PublishError::Domain(
                ProfileStoreError::ReferenceUnavailable,
            ));
        }
        if !names.insert(revision.content.name) {
            return Err(PublishError::Domain(
                ProfileError::new("duplicate_workspace_name", "workspace_profiles").into(),
            ));
        }
    }
    Ok(())
}

async fn find(
    collection: &Collection<Document>,
    filter: Document,
    session: &mut ClientSession,
) -> Result<Document, PublishError> {
    collection
        .find_one(filter)
        .session(session)
        .await
        .map_err(PublishError::Driver)?
        .ok_or(PublishError::Domain(
            ProfileStoreError::ReferenceUnavailable,
        ))
}
