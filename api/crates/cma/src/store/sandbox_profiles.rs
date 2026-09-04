//! Mongo persistence for sandbox profile drafts and revisions.

use std::collections::BTreeSet;

use layer2_core::ids::{SandboxProfileId, SubjectId};
use layer2_core::profiles::store::{
    ProfilePage, ProfilePageRequest, ProfileStoreError, SandboxProfileStore,
};
use layer2_core::profiles::{
    content_digest, ProfileCapacityBounds, ProfileError, SandboxProfile, SandboxProfileRevision,
    WorkspaceProfileRevision,
};
use mongodb::bson::Document;

use super::profiles::{
    advance_publication, decode_record, get_revision, list_profiles, list_revisions,
    profile_document, readable_revision, retire_profile, revision_document, save_draft,
    ProfileDocumentInput, PublishError,
};
use super::sandbox_mongo::{
    commit_transaction, i64_field_matches, is_duplicate_key, is_transient, nullable, record_bson,
    signed, start_transaction, string_field_matches, CommitOutcome, MongoProfileStore,
    TRANSACTION_ATTEMPTS,
};

const KIND: &str = "sandbox";

fn input(profile: &SandboxProfile) -> ProfileDocumentInput<'_, SandboxProfile> {
    ProfileDocumentInput {
        id: profile.id.as_str(),
        owner: &profile.owner,
        visibility: profile.visibility,
        kind: KIND,
        draft_version: profile.draft_version,
        updated_at: &profile.updated_at,
        retired_at: profile.retired_at.as_ref(),
        record: profile,
    }
}

fn document(profile: &SandboxProfile) -> Result<Document, ProfileStoreError> {
    profile_document(&input(profile))
}

fn read_profile(document: &Document) -> Result<SandboxProfile, ProfileStoreError> {
    let profile: SandboxProfile = decode_record(document)?;
    let visibility = record_bson(&profile.visibility).map_err(ProfileStoreError::Corrupt)?;
    let draft_version = signed(profile.draft_version, "sandbox profile draft version")
        .map_err(ProfileStoreError::Corrupt)?;
    if !string_field_matches(document, "_id", profile.id.as_str())
        || !string_field_matches(document, "owner", profile.owner.as_str())
        || document.get("visibility") != Some(&visibility)
        || !string_field_matches(document, "kind", KIND)
        || !i64_field_matches(document, "draft_version", draft_version)
        || !string_field_matches(document, "updated_at", &profile.updated_at)
        || document.get("retired_at") != Some(&nullable(profile.retired_at.as_ref()))
    {
        return Err(ProfileStoreError::Corrupt(
            "sandbox profile query fields disagree with its aggregate".into(),
        ));
    }
    Ok(profile)
}

fn read_revision(document: &Document) -> Result<SandboxProfileRevision, ProfileStoreError> {
    let revision: SandboxProfileRevision = decode_record(document)?;
    let digest = content_digest(&revision.content)?;
    if !string_field_matches(document, "profile_id", revision.profile_id.as_str())
        || !i64_field_matches(document, "revision", i64::from(revision.revision))
        || digest != revision.digest
    {
        return Err(ProfileStoreError::Corrupt(
            "sandbox profile revision fields or digest disagree".into(),
        ));
    }
    Ok(revision)
}

fn read_workspace_reference(
    document: &Document,
    profile_id: &str,
    revision: u32,
) -> Result<WorkspaceProfileRevision, ProfileStoreError> {
    let stored: WorkspaceProfileRevision = decode_record(document)?;
    if stored.profile_id.as_str() != profile_id
        || stored.revision != revision
        || content_digest(&stored.content)? != stored.digest
    {
        return Err(ProfileStoreError::Corrupt(
            "referenced workspace profile revision is inconsistent".into(),
        ));
    }
    Ok(stored)
}

#[async_trait::async_trait]
impl SandboxProfileStore for MongoProfileStore {
    async fn create_sandbox_profile(
        &self,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError> {
        if profile.draft_version == 0 {
            return Err(ProfileStoreError::Conflict);
        }
        self.sandbox_profiles
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

    async fn save_sandbox_draft(
        &self,
        expected: u64,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError> {
        save_draft(&self.sandbox_profiles, expected, input(&profile)).await
    }

    async fn publish_sandbox_profile(
        &self,
        id: &SandboxProfileId,
        expected: u64,
        published_at: String,
        bounds: ProfileCapacityBounds,
        secret_patterns: &[String],
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
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
                &self.sandbox_profiles,
                &mut session,
                id.as_str(),
                expected,
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
            let mut names = BTreeSet::new();
            let mut reference_error = None;
            for reference in &profile.draft.workspace_profiles {
                match readable_revision(
                    &self.workspace_revisions,
                    &self.workspace_profiles,
                    reference.id.as_str(),
                    reference.revision,
                    &profile.owner,
                    &mut session,
                )
                .await
                {
                    Ok(document) => match read_workspace_reference(
                        &document,
                        reference.id.as_str(),
                        reference.revision,
                    ) {
                        Ok(revision) if names.insert(revision.content.name.clone()) => {}
                        Ok(_) => {
                            reference_error = Some(PublishError::Domain(
                                ProfileError::new("duplicate_workspace_name", "workspace_profiles")
                                    .into(),
                            ));
                            break;
                        }
                        Err(error) => {
                            reference_error = Some(PublishError::Domain(error));
                            break;
                        }
                    },
                    Err(error) => {
                        reference_error = Some(error);
                        break;
                    }
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
            let revision =
                match profile.publish(number, published_at.clone(), bounds, secret_patterns) {
                    Ok(revision) => revision,
                    Err(error) => {
                        let _ = session.abort_transaction().await;
                        return Err(error.into());
                    }
                };
            let inserted = self
                .sandbox_revisions
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
                        &self.sandbox_revisions,
                        id.as_str(),
                        number,
                        read_revision,
                    )
                    .await?
                    {
                        Some(stored) if stored == revision => Ok(stored),
                        _ => Err(ProfileStoreError::Unavailable(format!(
                            "sandbox profile publication commit remained unknown: {error}"
                        ))),
                    };
                }
                CommitOutcome::Failed(error) => {
                    return Err(ProfileStoreError::Unavailable(error.to_string()));
                }
            }
        }
        Err(ProfileStoreError::Unavailable(format!(
            "sandbox profile publication exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }

    async fn sandbox_profile_revision(
        &self,
        id: &SandboxProfileId,
        revision: u32,
    ) -> Result<Option<SandboxProfileRevision>, ProfileStoreError> {
        get_revision(
            &self.sandbox_revisions,
            id.as_str(),
            revision,
            read_revision,
        )
        .await
    }

    async fn list_sandbox_profile_revisions(
        &self,
        id: &SandboxProfileId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<SandboxProfileRevision>, ProfileStoreError> {
        list_revisions(
            &self.sandbox_profiles,
            &self.sandbox_revisions,
            id.as_str(),
            page,
            read_revision,
        )
        .await
    }

    async fn list_sandbox_profiles(
        &self,
        owner: Option<&SubjectId>,
        public_only: bool,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<SandboxProfile>, ProfileStoreError> {
        list_profiles(
            &self.sandbox_profiles,
            owner,
            public_only,
            page,
            read_profile,
        )
        .await
    }

    async fn retire_sandbox_profile(
        &self,
        id: &SandboxProfileId,
        owner: &SubjectId,
        retired_at: String,
    ) -> Result<(), ProfileStoreError> {
        retire_profile(&self.sandbox_profiles, id.as_str(), owner, retired_at).await
    }
}
