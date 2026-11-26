//! Mongo persistence for sandbox profile drafts and revisions.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::ids::{SandboxProfileId, SubjectId};
use layer2_core::profiles::store::{
    ProfilePage, ProfilePageRequest, ProfileStoreError, SandboxProfileStore,
};
use layer2_core::profiles::{
    content_digest, ProfileCapacityBounds, SandboxProfile, SandboxProfileRevision,
};
use mongodb::bson::Document;

use super::profile_documents::{
    decode_record, profile_document, revision_document, ProfileDocumentInput,
};
use super::profiles::{
    advance_publication, get_profile, get_revision, list_profiles, list_revisions, retire_profile,
    PublishError,
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

pub(super) fn document(profile: &SandboxProfile) -> Result<Document, ProfileStoreError> {
    profile_document(&input(profile))
}

pub(super) fn read_profile(document: &Document) -> Result<SandboxProfile, ProfileStoreError> {
    let profile: SandboxProfile = decode_record(document)?;
    profile.availability.validate(profile.visibility)?;
    let visibility = record_bson(&profile.visibility).map_err(ProfileStoreError::Corrupt)?;
    let draft_version = signed(profile.draft_version, "sandbox profile draft version")
        .map_err(ProfileStoreError::Corrupt)?;
    let uses_count = signed(profile.uses_count, "sandbox profile use count")
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
            "sandbox profile query fields disagree with its aggregate".into(),
        ));
    }
    Ok(profile)
}

pub(super) fn read_revision(
    document: &Document,
) -> Result<SandboxProfileRevision, ProfileStoreError> {
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

#[async_trait::async_trait]
impl SandboxProfileStore for MongoProfileStore {
    async fn create_sandbox_profile(
        &self,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError> {
        profile.availability.validate(profile.visibility)?;
        if profile.availability.warmup_enabled {
            return Err(layer2_core::profiles::ProfileError::new(
                "warmup_requires_published_profile",
                "availability.warmup_enabled",
            )
            .into());
        }
        if profile.draft_version != 1 || profile.forked_from.is_some() {
            return Err(ProfileStoreError::Conflict);
        }
        super::profile_draft_admission::create(
            self,
            &self.sandbox_profiles,
            document(&profile)?,
            super::profile_draft_admission::Pins::sandbox(&profile),
        )
        .await
    }

    async fn sandbox_profile(
        &self,
        id: &SandboxProfileId,
    ) -> Result<Option<SandboxProfile>, ProfileStoreError> {
        get_profile(&self.sandbox_profiles, id.as_str(), read_profile).await
    }

    async fn save_sandbox_draft(
        &self,
        expected: u64,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError> {
        profile.availability.validate(profile.visibility)?;
        super::profile_draft_admission::save(
            self,
            &self.sandbox_profiles,
            expected,
            input(&profile),
            super::profile_draft_admission::Pins::sandbox(&profile),
        )
        .await
    }

    async fn publish_sandbox_profile(
        &self,
        id: &SandboxProfileId,
        expected: u64,
        published_at: String,
        bounds: ProfileCapacityBounds,
        scanner: &dyn layer2_core::profiles::PublicValueScanner,
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        self.publish_sandbox_with_initial(id, expected, published_at, None, bounds, scanner)
            .await
    }

    async fn ensure_initial_sandbox_profile(
        &self,
        profile: &SandboxProfile,
        published_at: String,
        bounds: ProfileCapacityBounds,
        scanner: &dyn layer2_core::profiles::PublicValueScanner,
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        profile.validate(bounds)?;
        if profile.draft_version != 1
            || profile.retired_at.is_some()
            || profile.visibility != layer2_core::profiles::ProfileVisibility::Private
            || profile.forked_from.is_some()
        {
            return Err(ProfileStoreError::Conflict);
        }
        self.publish_sandbox_with_initial(
            &profile.id,
            1,
            published_at,
            Some(profile),
            bounds,
            scanner,
        )
        .await
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

    async fn list_sandbox_profile_summaries(
        &self,
        owner: &layer2_core::SubjectId,
        page: &ProfilePageRequest,
    ) -> Result<
        ProfilePage<layer2_core::profiles::store::ProfileSummary<SandboxProfile>>,
        ProfileStoreError,
    > {
        super::profiles::summaries::SummaryReader {
            client: &self.client,
            profiles: &self.sandbox_profiles,
            revisions: &self.sandbox_revisions,
            profile: read_profile,
            revision: |row| {
                read_revision(row).map(|revision| {
                    layer2_core::profiles::store::PublishedProfileSummary::from(&revision)
                })
            },
        }
        .read(owner, page)
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

impl MongoProfileStore {
    async fn publish_sandbox_with_initial(
        &self,
        id: &SandboxProfileId,
        expected: u64,
        published_at: String,
        initial: Option<&SandboxProfile>,
        bounds: ProfileCapacityBounds,
        scanner: &dyn layer2_core::profiles::PublicValueScanner,
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        let mut last_transient = None;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            super::profiles::retry_pause(attempt, initial.is_some()).await;
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
                &published_at,
                initial.is_some(),
            )
            .await
            {
                Ok(value) => value,
                Err(PublishError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    if let Some(initial) = initial.filter(|_| error == ProfileStoreError::Conflict)
                    {
                        return self.replay_initial_sandbox(initial).await;
                    }
                    return Err(error);
                }
                Err(PublishError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(PublishError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(ProfileStoreError::Unavailable(error.to_string()));
                }
            };
            let profile = match read_profile(&profile) {
                Ok(profile) => profile,
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
            };
            if let Err(error) =
                super::profile_references::sandbox_references(self, &profile, &mut session).await
            {
                let _ = session.abort_guarded().await;
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
            if initial.is_some_and(|expected| expected != &profile) {
                let _ = session.abort_guarded().await;
                return Err(ProfileStoreError::Conflict);
            }
            let revision = match profile
                .publish(number, published_at.clone(), bounds, scanner)
                .await
            {
                Ok(revision) => revision,
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    return Err(error.into());
                }
            };
            let inserted = self
                .sandbox_revisions
                .insert_one(revision_document(id.as_str(), number, &revision)?)
                .session(&mut session)
                .await;
            if let Err(error) = inserted {
                let _ = session.abort_guarded().await;
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
                    return match super::profiles::get_revision_after_unknown_commit(
                        &mut session,
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

    async fn replay_initial_sandbox(
        &self,
        expected: &SandboxProfile,
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        if self.sandbox_profile(&expected.id).await?.as_ref() != Some(expected) {
            return Err(ProfileStoreError::Conflict);
        }
        let revision = self
            .sandbox_profile_revision(&expected.id, 1)
            .await?
            .ok_or(ProfileStoreError::Conflict)?;
        if revision.profile_id != expected.id
            || revision.revision != 1
            || revision.content != expected.draft
            || revision.digest != content_digest(&expected.draft)?
        {
            return Err(ProfileStoreError::Conflict);
        }
        Ok(revision)
    }
}
