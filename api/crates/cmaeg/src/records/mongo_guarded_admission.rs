//! Commit a channel-guarded admission and any new permanent partition assignment.
use super::*;
use crate::records::AuthorityGuard;
use crate::transaction_terminal::GuardedTerminal as _;

impl MongoRecords {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn guarded_insert_interaction(
        &self,
        id: &str,
        owner: &str,
        blob: &[u8],
        expires_at: i64,
        channel_record_id: &str,
        channel_version: u64,
        authority_guards: &[AuthorityGuard],
        projection: Option<ReplicaProjection>,
        assignment: Option<&ReplicaWrite>,
    ) -> Result<u64, RecordError> {
        if self.kind != RecordKind::Interaction
            || id.is_empty()
            || owner.is_empty()
            || blob.is_empty()
            || blob.len() > 65_536
        {
            return Err(RecordError::Rejected("invalid guarded interaction".into()));
        }
        let expiry = DateTime::from_millis(
            expires_at
                .checked_mul(1000)
                .ok_or_else(|| RecordError::Rejected("invalid interaction expiry".into()))?,
        );
        let mut session = self.db.client().start_session().await.map_err(failure)?;
        crate::transaction_start::start_write(&self.db, &mut session, None)
            .await
            .map_err(failure)?;
        let channel = self.db.collection::<Document>(RECORDS);
        let guarded = channel
            .update_one(
                doc! {
                    "_id": channel_record_id,
                    "owner": owner,
                    "version": i64::try_from(channel_version)
                        .map_err(|_| RecordError::Rejected("invalid channel version".into()))?,
                },
                doc! {"$inc":{"guard_revision":1_i64}},
            )
            .session(&mut session)
            .await
            .map_err(failure)?;
        if guarded.matched_count != 1 {
            return Err(RecordError::GuardConflict {
                scope: "channel",
                expected: channel_version,
            });
        }
        let authorities = self.db.collection::<Document>("eg_automation_records");
        for guard in authority_guards {
            let changed = authorities
                .update_one(
                    doc! {
                        "_id": &guard.id,
                        "owner": &guard.owner,
                        "kind": guard.kind,
                        "version": i64::try_from(guard.version).map_err(|_| {
                            RecordError::Rejected("invalid authority version".into())
                        })?,
                    },
                    doc! {"$inc":{"guard_revision":1_i64}},
                )
                .session(&mut session)
                .await
                .map_err(failure)?;
            if changed.matched_count != 1 {
                return Err(RecordError::GuardConflict {
                    scope: guard.kind,
                    expected: guard.version,
                });
            }
        }
        if let Some(assignment) = assignment {
            if assignment.owner != owner
                || assignment.id.is_empty()
                || assignment.expires_at.is_some()
                || assignment.blob.is_empty()
                || assignment.blob.len() > 65536
                || assignment.channel.as_ref()
                    != Some(&(channel_record_id.to_owned(), channel_version))
                || assignment.condition.is_some()
                || assignment.guard.is_some()
                || assignment.projection.is_some()
            {
                return Err(RecordError::Rejected("invalid partition assignment".into()));
            }
            if let Some(version) = assignment.expected {
                let updated = self.db.collection::<Document>(CONNECTIONS).update_one(
                    doc! {"_id": &assignment.id, "owner": owner,
                        "version": i64::try_from(version).map_err(|_| RecordError::Rejected("invalid partition assignment version".into()))?},
                    doc! {"$inc": {"guard_revision": 1_i64}},
                ).session(&mut session).await.map_err(failure)?;
                if updated.matched_count != 1 {
                    return Err(RecordError::GuardConflict {
                        scope: "partition_assignment",
                        expected: version,
                    });
                }
            } else {
                self.db
                    .collection::<Record>(CONNECTIONS)
                    .insert_one(Record {
                        id: assignment.id.clone(),
                        owner: owner.into(),
                        blob: Binary {
                            subtype: BinarySubtype::Generic,
                            bytes: assignment.blob.clone(),
                        },
                        version: 1,
                        schema_version: 1,
                        guard_revision: 0,
                        replica: None,
                        expires_at: None,
                    })
                    .session(&mut session)
                    .await
                    .map_err(failure)?;
            }
        }
        let row = Record {
            id: id.into(),
            owner: owner.into(),
            blob: Binary {
                subtype: BinarySubtype::Generic,
                bytes: blob.to_vec(),
            },
            version: 1,
            schema_version: 1,
            guard_revision: 0,
            replica: projection,
            expires_at: Some(expiry),
        };
        self.records
            .insert_one(row)
            .session(&mut session)
            .await
            .map_err(failure)?;
        session.commit_guarded().await.map_err(failure)?;
        Ok(1)
    }
}
