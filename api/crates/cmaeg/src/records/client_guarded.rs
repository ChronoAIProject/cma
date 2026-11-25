use super::{AuthorityGuard, RecordClient, RecordError};

impl RecordClient {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn put_interaction_guarded(
        &self,
        id: &str,
        owner: &str,
        blob: &[u8],
        expires_at: i64,
        channel_record_id: &str,
        channel_version: u64,
        authority_guards: &[AuthorityGuard],
    ) -> Result<u64, RecordError> {
        let database = self.database.as_ref().ok_or_else(|| {
            RecordError::Unavailable("guarded admission requires local records".into())
        })?;
        database
            .guarded_insert_interaction(
                id,
                owner,
                blob,
                expires_at,
                channel_record_id,
                channel_version,
                authority_guards,
                None,
                None,
            )
            .await
    }
}
impl super::RecordClient {
    pub(crate) async fn replica_write(
        &self,
        writes: &[crate::records::mongo::ReplicaWrite],
    ) -> Result<Vec<u64>, super::RecordError> {
        let database = self.database.as_ref().ok_or_else(|| {
            super::RecordError::Rejected("replica ownership requires direct Mongo storage".into())
        })?;
        database
            .for_kind(self.record_kind)
            .replica_write(writes)
            .await
    }
}

/// Only the immutable routed root receives the indexed admission projection.
pub(crate) struct AdmissionRootWrite<'a> {
    pub id: &'a str,
    pub owner: &'a str,
    pub blob: &'a [u8],
    pub expires_at: i64,
    pub deployment: &'a str,
    pub github_guard: Option<(&'a str, u64, &'a [AuthorityGuard])>,
}
impl RecordClient {
    pub(crate) async fn put_admission_root(
        &self,
        write: AdmissionRootWrite<'_>,
    ) -> Result<u64, RecordError> {
        use crate::records::mongo::{ReplicaKind, ReplicaProjection, ReplicaWrite};
        self.record_url(write.id)?;
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| RecordError::Rejected("indexed admission requires direct Mongo".into()))?
            .for_kind(crate::records::RecordKind::Interaction);
        let projection = ReplicaProjection {
            kind: ReplicaKind::Admission,
            deployment: write.deployment.into(),
            active: true,
        };
        if let Some((channel, version, guards)) = write.github_guard {
            database
                .guarded_insert_interaction(
                    write.id,
                    write.owner,
                    write.blob,
                    write.expires_at,
                    channel,
                    version,
                    guards,
                    Some(projection),
                    None,
                )
                .await
        } else {
            let versions = database
                .replica_write(&[ReplicaWrite {
                    id: write.id.into(),
                    owner: write.owner.into(),
                    blob: write.blob.to_vec(),
                    expected: None,
                    expires_at: Some(write.expires_at),
                    condition: None,
                    guard: None,
                    channel: None,
                    projection: Some(projection),
                }])
                .await?;
            versions
                .first()
                .copied()
                .ok_or_else(|| RecordError::Malformed("missing admission version".into()))
        }
    }
    pub(crate) async fn finish_admission_index(
        &self,
        id: &str,
        owner: &str,
        version: u64,
        deployment: &str,
    ) -> Result<(), RecordError> {
        self.database
            .as_ref()
            .ok_or_else(|| RecordError::Rejected("indexed admission requires direct Mongo".into()))?
            .finish_admission_index(id, owner, version, deployment)
            .await
    }
    pub(crate) async fn list_admission_roots(
        &self,
        deployment: &str,
        owner: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<super::RecordHead>, RecordError> {
        self.database
            .as_ref()
            .ok_or_else(|| RecordError::Rejected("indexed admission requires direct Mongo".into()))?
            .list_admission_roots(deployment, owner, after, limit)
            .await
    }
}

impl RecordClient {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn put_profile_admission(
        &self,
        id: &str,
        owner: &str,
        blob: &[u8],
        expires_at: i64,
        profile: &crate::channels::profile_assignment::Admission,
        guards: &[AuthorityGuard],
        projection: Option<crate::records::mongo::ReplicaProjection>,
    ) -> Result<u64, RecordError> {
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| {
                RecordError::Unavailable("profile admission requires local records".into())
            })?
            .for_kind(crate::records::RecordKind::Interaction);
        self.record_url(id)?;
        database
            .guarded_insert_interaction(
                id,
                owner,
                blob,
                expires_at,
                &profile.channel_record_id,
                profile.channel_version,
                guards,
                projection,
                profile.insert.as_ref(),
            )
            .await
    }
}
