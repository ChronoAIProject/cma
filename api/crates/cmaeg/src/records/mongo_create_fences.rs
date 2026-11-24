//! Serialize creator cancellation with the first durable Channel reservation.
use super::{failure, MongoRecords, Record, RecordError, RecordKind, CONNECTIONS};
use futures_util::FutureExt;
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, Document},
    error::{Error, ErrorKind, WriteFailure},
    options::{Acknowledgment, ReadConcern, ReturnDocument, WriteConcern},
    ClientSession, Database,
};
use serde::Deserialize;
use std::time::Duration;

pub(super) const CREATE_FENCES: &str = "eg_channel_create_fences";
const TRANSACTION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) struct CreateFenceIdentity<'a> {
    pub issuer: &'a str,
    pub kind: &'static str,
    pub subject: &'a str,
    pub record: &'a str,
}

impl CreateFenceIdentity<'_> {
    fn key(&self) -> Document {
        doc! {"issuer": self.issuer, "kind": self.kind, "subject": self.subject, "record": self.record}
    }

    fn valid(&self) -> bool {
        !self.issuer.is_empty()
            && self.issuer.len() <= 512
            && !self.subject.is_empty()
            && self.subject.len() <= 512
            && uuid::Uuid::parse_str(self.record).is_ok_and(|id| id.to_string() == self.record)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancelCreate {
    FencedAbsent,
    RecordPresent,
}

pub(crate) struct CreateConnectionClaim {
    pub record_id: String,
    pub expected_revision: u64,
    pub sealed_payload: Vec<u8>,
}

#[derive(Deserialize)]
struct Fence {
    cancelled: bool,
}

impl MongoRecords {
    pub(crate) async fn reserve_channel_create(
        &self,
        identity: &CreateFenceIdentity<'_>,
        blob: &[u8],
        connection: Option<&CreateConnectionClaim>,
    ) -> Result<u64, RecordError> {
        if self.kind != RecordKind::Channel
            || !identity.valid()
            || blob.is_empty()
            || blob.len() > 65536
        {
            return Err(RecordError::Rejected("invalid Channel reservation".into()));
        }
        if let Some(tracking) = &self.worker_credentials {
            tracking.record_blob(blob).await?;
        }
        let channel = Record {
            id: identity.record.into(),
            owner: identity.subject.into(),
            blob: Binary {
                subtype: BinarySubtype::Generic,
                bytes: blob.to_vec(),
            },
            version: 1,
            schema_version: 1,
            guard_revision: 1,
            replica: None,
            expires_at: None,
        };
        let result = tokio::time::timeout(TRANSACTION_TIMEOUT, async {
            let mut session = self.db.client().start_session().await?;
            session
                .start_transaction()
                .read_concern(ReadConcern::snapshot())
                .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
                .and_run(
                    (&self.db, &self.records, identity, &channel, connection),
                    |session, (db, records, identity, channel, connection)| {
                        async move {
                            if guard(db, identity, session).await?.cancelled {
                                return Err(Error::custom(RecordError::GuardConflict {
                                    scope: "channel_create",
                                    expected: 0,
                                }));
                            }
                            if let Some(connection) = connection {
                                claim_connection(db, identity, connection, session).await?;
                            }
                            records.insert_one(*channel).session(session).await?;
                            Ok(1)
                        }
                        .boxed()
                    },
                )
                .await
        })
        .await
        .map_err(deadline)?;
        match result {
            Ok(version) => Ok(version),
            Err(error) => {
                if let Some(rejected) = error.get_custom::<RecordError>() {
                    return Err(rejected.clone());
                }
                if matches!(error.kind.as_ref(), ErrorKind::Write(WriteFailure::WriteError(write)) if write.code == 11000)
                {
                    return Err(self.conflict(identity.record, 0).await?);
                }
                Err(failure(error))
            }
        }
    }

    pub(crate) async fn cancel_channel_create(
        &self,
        identity: &CreateFenceIdentity<'_>,
    ) -> Result<CancelCreate, RecordError> {
        if self.kind != RecordKind::Channel || !identity.valid() {
            return Err(RecordError::Rejected("invalid Channel cancellation".into()));
        }
        let result = tokio::time::timeout(TRANSACTION_TIMEOUT, async {
            let mut session = self.db.client().start_session().await?;
            session
                .start_transaction()
                .read_concern(ReadConcern::snapshot())
                .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
                .and_run(
                    (&self.db, &self.records, identity),
                    |session, (db, records, identity)| {
                        async move {
                            let fence = guard(db, identity, session).await?;
                            if records
                                .find_one(doc! {"_id": identity.record, "owner": identity.subject})
                                .session(&mut *session)
                                .await?
                                .is_some()
                            {
                                return Err(Error::custom(CancelCreate::RecordPresent));
                            }
                            if !fence.cancelled {
                                db.collection::<Document>(CREATE_FENCES)
                                    .update_one(
                                        doc! {"_id": identity.key()},
                                        doc! {"$set": {"cancelled": true}},
                                    )
                                    .session(session)
                                    .await?;
                            }
                            Ok(CancelCreate::FencedAbsent)
                        }
                        .boxed()
                    },
                )
                .await
        })
        .await
        .map_err(deadline)?;
        result.or_else(|error| {
            error
                .get_custom::<CancelCreate>()
                .copied()
                .ok_or_else(|| failure(error))
        })
    }
}

fn deadline(_: tokio::time::error::Elapsed) -> RecordError {
    RecordError::Unavailable("Channel create transaction deadline exceeded".into())
}

async fn guard(
    database: &Database,
    identity: &CreateFenceIdentity<'_>,
    session: &mut ClientSession,
) -> mongodb::error::Result<Fence> {
    database
        .collection::<Fence>(CREATE_FENCES)
        .find_one_and_update(
            doc! {"_id": identity.key()},
            doc! {"$inc": {"revision": 1_i64}, "$setOnInsert": {"cancelled": false}},
        )
        .upsert(true)
        .return_document(ReturnDocument::After)
        .session(session)
        .await?
        .ok_or_else(|| Error::custom("Channel create guard could not be acquired"))
}

async fn claim_connection(
    database: &Database,
    identity: &CreateFenceIdentity<'_>,
    claim: &CreateConnectionClaim,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    let conflict = || {
        Error::custom(RecordError::GuardConflict {
            scope: "connection",
            expected: claim.expected_revision,
        })
    };
    let expected = i64::try_from(claim.expected_revision)
        .ok()
        .filter(|value| (1..i64::MAX).contains(value))
        .ok_or_else(conflict)?;
    let blob = Binary {
        subtype: BinarySubtype::Generic,
        bytes: claim.sealed_payload.clone(),
    };
    let result = database
        .collection::<Document>(CONNECTIONS)
        .update_one(
            doc! {"_id": &claim.record_id, "owner": identity.subject, "version": expected},
            doc! {"$set": {"blob": blob}, "$inc": {"version": 1_i64}},
        )
        .session(session)
        .await?;
    if result.matched_count != 1 {
        return Err(conflict());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires a disposable Mongo replica set"]
    async fn creator_cancellation_and_initial_reservation_share_a_permanent_fence() {
        let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
        let first = mongodb::Client::with_uri_str(&uri).await.unwrap();
        let second = mongodb::Client::with_uri_str(&uri).await.unwrap();
        let db_name = format!("create_fence_{}", uuid::Uuid::new_v4().simple());
        let db = first.database(&db_name);
        super::super::migrate(&db).await.unwrap();
        let issuer = "https://issuer.example";
        super::super::migrate_create_fence_floor(&db, issuer)
            .await
            .unwrap();
        super::super::migrate(&db).await.unwrap();
        let a = MongoRecords::read_failure_fixture(db.clone());
        let b = MongoRecords::read_failure_fixture(second.database(&db_name));
        let legacy_record = uuid::Uuid::new_v4().to_string();
        assert!(matches!(
            a.put(&legacy_record, "alice", b"old writer", None).await,
            Err(RecordError::Unavailable(_)),
        ));
        assert_eq!(b.get(&legacy_record).await, Err(RecordError::NotFound));
        let missing = uuid::Uuid::new_v4().to_string();
        let alice = CreateFenceIdentity {
            issuer,
            kind: "human",
            subject: "alice",
            record: &missing,
        };
        let bob = CreateFenceIdentity {
            subject: "bob",
            ..alice
        };
        assert_eq!(
            a.cancel_channel_create(&alice).await.unwrap(),
            CancelCreate::FencedAbsent,
        );
        let connection = uuid::Uuid::new_v4().to_string();
        let connection_rows = a.for_kind(RecordKind::Connection);
        connection_rows
            .put(&connection, "alice", b"before claim", None)
            .await
            .unwrap();
        let claim = CreateConnectionClaim {
            record_id: connection.clone(),
            expected_revision: 1,
            sealed_payload: b"after claim".to_vec(),
        };
        assert_eq!(
            b.reserve_channel_create(&alice, b"fixture", Some(&claim))
                .await,
            Err(RecordError::GuardConflict {
                scope: "channel_create",
                expected: 0
            }),
        );
        assert_eq!(connection_rows.get(&connection).await.unwrap().version, 1);
        assert_eq!(
            b.reserve_channel_create(&bob, b"fixture", None)
                .await
                .unwrap(),
            1
        );
        assert!(matches!(
            b.reserve_channel_create(&alice, b"fixture", Some(&claim))
                .await,
            Err(RecordError::GuardConflict {
                scope: "channel_create",
                expected: 0
            }),
        ));
        assert_eq!(connection_rows.get(&connection).await.unwrap().version, 1);
        assert_eq!(
            a.cancel_channel_create(&bob).await.unwrap(),
            CancelCreate::RecordPresent,
        );
        assert_eq!(
            b.cancel_channel_create(&alice).await.unwrap(),
            CancelCreate::FencedAbsent,
        );
        assert!(b.delete(&missing, 1).await.unwrap());
        assert_eq!(
            a.cancel_channel_create(&bob).await.unwrap(),
            CancelCreate::FencedAbsent,
        );
        assert_eq!(
            b.reserve_channel_create(&bob, b"fixture", None).await,
            Err(RecordError::GuardConflict {
                scope: "channel_create",
                expected: 0
            }),
        );
        let live = uuid::Uuid::new_v4().to_string();
        let admitted = CreateFenceIdentity {
            record: &live,
            ..alice
        };
        assert_eq!(
            a.reserve_channel_create(&admitted, b"fixture", Some(&claim))
                .await
                .unwrap(),
            1,
        );
        assert_eq!(
            connection_rows.get(&connection).await.unwrap().blob,
            b"after claim"
        );
        let another = uuid::Uuid::new_v4().to_string();
        let stale = CreateFenceIdentity {
            record: &another,
            ..alice
        };
        assert_eq!(
            b.reserve_channel_create(&stale, b"fixture", Some(&claim))
                .await,
            Err(RecordError::GuardConflict {
                scope: "connection",
                expected: 1
            }),
        );
        assert_eq!(a.get(&another).await, Err(RecordError::NotFound));

        for _ in 0..16 {
            let id = uuid::Uuid::new_v4().to_string();
            let racing = CreateFenceIdentity {
                record: &id,
                ..alice
            };
            let (reserved, cancelled) = tokio::join!(
                a.reserve_channel_create(&racing, b"racing create", None),
                b.cancel_channel_create(&racing),
            );
            match cancelled.unwrap() {
                CancelCreate::FencedAbsent => {
                    assert_eq!(
                        reserved,
                        Err(RecordError::GuardConflict {
                            scope: "channel_create",
                            expected: 0,
                        }),
                    );
                    assert_eq!(a.get(&id).await, Err(RecordError::NotFound));
                }
                CancelCreate::RecordPresent => {
                    assert_eq!(reserved.unwrap(), 1);
                    assert_eq!(a.get(&id).await.unwrap().blob, b"racing create");
                }
            }
        }
        db.drop().await.unwrap();
    }
}
