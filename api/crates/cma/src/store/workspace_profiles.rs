//! Mongo persistence for workspace profile drafts and revisions.

use layer2_core::ids::{SubjectId, WorkspaceProfileId};
use layer2_core::profiles::store::{
    ProfilePage, ProfilePageRequest, ProfileStoreError, WorkspaceProfileStore,
};
use layer2_core::profiles::{content_digest, WorkspaceProfile, WorkspaceProfileRevision};
use mongodb::bson::Document;

use super::profile_documents::{
    decode_record, profile_document, revision_document, save_draft, ProfileDocumentInput,
};
use super::profiles::{
    advance_publication, get_profile, get_revision, list_profiles, list_revisions,
    readable_revision, retire_profile, PublishError,
};
use super::sandbox_mongo::{
    commit_transaction, i64_field_matches, is_duplicate_key, is_transient, nullable, record_bson,
    signed, start_transaction, string_field_matches, CommitOutcome, MongoProfileStore,
    TRANSACTION_ATTEMPTS,
};

const KIND: &str = "workspace";

fn input(profile: &WorkspaceProfile) -> ProfileDocumentInput<'_, WorkspaceProfile> {
    ProfileDocumentInput {
        id: profile.id.as_str(),
        owner: &profile.owner,
        name: &profile.name,
        description: &profile.description,
        visibility: profile.visibility,
        kind: KIND,
        uses_count: profile.uses_count,
        draft_version: profile.draft_version,
        updated_at: &profile.updated_at,
        retired_at: profile.retired_at.as_ref(),
        record: profile,
    }
}

fn document(profile: &WorkspaceProfile) -> Result<Document, ProfileStoreError> {
    profile_document(&input(profile))
}

pub(super) fn read_profile(document: &Document) -> Result<WorkspaceProfile, ProfileStoreError> {
    let profile: WorkspaceProfile = decode_record(document)?;
    profile.validate()?;
    let visibility = record_bson(&profile.visibility).map_err(ProfileStoreError::Corrupt)?;
    let draft_version = signed(profile.draft_version, "workspace profile draft version")
        .map_err(ProfileStoreError::Corrupt)?;
    let uses_count = signed(profile.uses_count, "workspace profile use count")
        .map_err(ProfileStoreError::Corrupt)?;
    if !string_field_matches(document, "_id", profile.id.as_str())
        || !string_field_matches(document, "owner", profile.owner.as_str())
        || !string_field_matches(document, "name", &profile.name)
        || !string_field_matches(document, "description", &profile.description)
        || document.get("visibility") != Some(&visibility)
        || !string_field_matches(document, "kind", KIND)
        || !i64_field_matches(document, "uses_count", uses_count)
        || !i64_field_matches(document, "draft_version", draft_version)
        || !string_field_matches(document, "updated_at", &profile.updated_at)
        || document.get("retired_at") != Some(&nullable(profile.retired_at.as_ref()))
    {
        return Err(ProfileStoreError::Corrupt(
            "workspace profile query fields disagree with its aggregate".into(),
        ));
    }
    Ok(profile)
}

pub(super) fn read_revision(
    document: &Document,
) -> Result<WorkspaceProfileRevision, ProfileStoreError> {
    let revision: WorkspaceProfileRevision = decode_record(document)?;
    let digest = content_digest(&revision.content)?;
    if !string_field_matches(document, "profile_id", revision.profile_id.as_str())
        || !i64_field_matches(document, "revision", i64::from(revision.revision))
        || digest != revision.digest
    {
        return Err(ProfileStoreError::Corrupt(
            "workspace profile revision fields or digest disagree".into(),
        ));
    }
    Ok(revision)
}

#[async_trait::async_trait]
impl WorkspaceProfileStore for MongoProfileStore {
    async fn create_workspace_profile(
        &self,
        profile: WorkspaceProfile,
    ) -> Result<(), ProfileStoreError> {
        profile.validate()?;
        if profile.draft_version != 1 {
            return Err(ProfileStoreError::Conflict);
        }
        self.workspace_profiles
            .insert_one(document(&profile)?)
            .await
            .map(|_| ())
            .map_err(|error| {
                if is_duplicate_key(&error) {
                    ProfileStoreError::Duplicate
                } else {
                    ProfileStoreError::Unavailable(error.to_string())
                }
            })
    }

    async fn workspace_profile(
        &self,
        id: &WorkspaceProfileId,
    ) -> Result<Option<WorkspaceProfile>, ProfileStoreError> {
        get_profile(&self.workspace_profiles, id.as_str(), read_profile).await
    }

    async fn save_workspace_draft(
        &self,
        expected: u64,
        profile: WorkspaceProfile,
    ) -> Result<(), ProfileStoreError> {
        profile.validate()?;
        save_draft(&self.workspace_profiles, expected, input(&profile)).await
    }

    async fn publish_workspace_profile(
        &self,
        id: &WorkspaceProfileId,
        expected: u64,
        published_at: String,
    ) -> Result<WorkspaceProfileRevision, ProfileStoreError> {
        let mut last_transient = None;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::task::yield_now().await;
            }
            let mut session = match self.client.start_session().await {
                Ok(session) => session,
                Err(error) if is_transient(&error) => {
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(error) => return Err(ProfileStoreError::Unavailable(error.to_string())),
            };
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    continue;
                }
                return Err(ProfileStoreError::Unavailable(error.to_string()));
            }
            let (profile, number) = match advance_publication(
                &self.workspace_profiles,
                &mut session,
                id.as_str(),
                expected,
                &published_at,
            )
            .await
            {
                Ok(value) => value,
                Err(PublishError::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(PublishError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(PublishError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(ProfileStoreError::Unavailable(error.to_string()));
                }
            };
            let profile = match read_profile(&profile) {
                Ok(profile) => profile,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
            };
            let mut reference_error = None;
            for reference in &profile.draft.agent_profiles {
                if let Err(error) = readable_revision(
                    &self.agent_revisions,
                    &self.agent_profiles,
                    reference.id.as_str(),
                    reference.revision,
                    &profile.owner,
                    &mut session,
                )
                .await
                {
                    reference_error = Some(error);
                    break;
                }
            }
            if let Some(error) = reference_error {
                let _ = session.abort_transaction().await;
                match error {
                    PublishError::Domain(error) => return Err(error),
                    PublishError::Driver(error) if is_transient(&error) => {
                        last_transient = Some(error.to_string());
                        continue;
                    }
                    PublishError::Driver(error) => {
                        return Err(ProfileStoreError::Unavailable(error.to_string()));
                    }
                }
            }
            let revision = match profile.publish(number, published_at.clone()) {
                Ok(revision) => revision,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    return Err(error.into());
                }
            };
            let inserted = self
                .workspace_revisions
                .insert_one(revision_document(id.as_str(), number, &revision)?)
                .session(&mut session)
                .await;
            if let Err(error) = inserted {
                let _ = session.abort_transaction().await;
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    continue;
                }
                return Err(if is_duplicate_key(&error) {
                    ProfileStoreError::Conflict
                } else {
                    ProfileStoreError::Unavailable(error.to_string())
                });
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(revision),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    return match get_revision(
                        &self.workspace_revisions,
                        id.as_str(),
                        number,
                        read_revision,
                    )
                    .await?
                    {
                        Some(stored) if stored == revision => Ok(stored),
                        _ => Err(ProfileStoreError::Unavailable(format!(
                            "workspace profile publication commit remained unknown: {error}"
                        ))),
                    };
                }
                CommitOutcome::Failed(error) => {
                    return Err(ProfileStoreError::Unavailable(error.to_string()));
                }
            }
        }
        Err(ProfileStoreError::Unavailable(format!(
            "workspace profile publication exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }

    async fn workspace_profile_revision(
        &self,
        id: &WorkspaceProfileId,
        revision: u32,
    ) -> Result<Option<WorkspaceProfileRevision>, ProfileStoreError> {
        get_revision(
            &self.workspace_revisions,
            id.as_str(),
            revision,
            read_revision,
        )
        .await
    }

    async fn list_workspace_profile_revisions(
        &self,
        id: &WorkspaceProfileId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<WorkspaceProfileRevision>, ProfileStoreError> {
        list_revisions(
            &self.workspace_profiles,
            &self.workspace_revisions,
            id.as_str(),
            page,
            read_revision,
        )
        .await
    }

    async fn list_workspace_profiles(
        &self,
        owner: Option<&SubjectId>,
        public_only: bool,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<WorkspaceProfile>, ProfileStoreError> {
        list_profiles(
            &self.workspace_profiles,
            owner,
            public_only,
            page,
            read_profile,
        )
        .await
    }

    async fn retire_workspace_profile(
        &self,
        id: &WorkspaceProfileId,
        owner: &SubjectId,
        retired_at: String,
    ) -> Result<(), ProfileStoreError> {
        retire_profile(&self.workspace_profiles, id.as_str(), owner, retired_at).await
    }
}
