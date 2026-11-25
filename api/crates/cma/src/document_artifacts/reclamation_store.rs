//! Bounded Mongo custody for generation-fenced source cleanup.
use super::{
    object_authority::{self, Authority, Lease, State, AUTHORITY_VERSION},
    quota,
    reclamation_binding::Binding,
    reclamation_slots,
    source_commit::source_marker,
    MongoDocumentStore,
};
use crate::document_storage::maintenance::ConfirmedDeletion;
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::{commit_transaction, is_confirmed_body_retry, start_transaction, CommitOutcome};
use futures_util::TryStreamExt;
use layer2_core::document_artifacts::{DocumentError, DocumentSourceReservation};
use mongodb::{
    bson::{doc, DateTime, Document},
    ClientSession, Collection, IndexModel,
};

#[path = "reclamation_identity.rs"]
mod identity;

const LIMIT: i64 = 32;
const ACCOUNTING_LIMIT: i64 = 4;
const AGE_MS: i64 = 60 * 60 * 1000;
const LEASE_MS: i64 = 90 * 1000;

#[derive(Debug)]
pub(super) struct DeleteClaim {
    pub source: DocumentSourceReservation,
    pub generation: u64,
    holder: String,
    binding: Binding,
}

pub(super) struct Candidate {
    pub source: DocumentSourceReservation,
    mode: CandidateMode,
}

pub(super) struct AccountingEntry {
    pub key: String,
    pub candidate: Option<Candidate>,
}

enum CandidateMode {
    Current(Authority),
    Legacy(Option<Document>),
}

pub(super) struct ReclamationStore {
    documents: MongoDocumentStore,
    ledger: Collection<Document>,
    namespace: String,
}

impl ReclamationStore {
    pub(super) fn new(documents: MongoDocumentStore, namespace: String) -> Self {
        let ledger = documents.quota(true).rows;
        Self {
            documents,
            ledger,
            namespace,
        }
    }

    pub(super) async fn ensure_indexes(&self) -> Result<(), DocumentError> {
        self.ledger.create_index(IndexModel::builder().keys(
            doc! { "kind": 1, "authority_version": 1, "state": 1, "claim_until": 1, "_id": 1 },
        ).build()).await.map_err(quota::unavailable)?;
        Ok(())
    }

    pub(super) async fn expired_claims(
        &self,
        now: DateTime,
    ) -> Result<Vec<Candidate>, DocumentError> {
        let mut candidates = Vec::new();
        let mut cursor = self
            .ledger
            .find(doc! {
                "kind": "source_commit", "authority_version": AUTHORITY_VERSION,
                "state": "delete_pending", "claim_until": { "$lte": now },
            })
            .sort(doc! { "claim_until": 1, "_id": 1 })
            .limit(LIMIT)
            .max_time(quota::IO)
            .await
            .map_err(quota::unavailable)?;
        while let Some(row) = cursor.try_next().await.map_err(quota::unavailable)? {
            let authority = Authority::decode(&row)?;
            candidates.push(Candidate {
                source: authority.source.clone(),
                mode: CandidateMode::Current(authority),
            });
        }
        Ok(candidates)
    }

    pub(super) async fn accounting_page(
        &self,
        after: Option<&str>,
        now: DateTime,
        legacy_mature: bool,
    ) -> Result<Vec<AccountingEntry>, DocumentError> {
        let mut rows = self
            .ledger
            .find(doc! {
                "kind": "object", "_id": { "$gt": after.unwrap_or("object:"), "$lt": "object;" },
            })
            .sort(doc! { "_id": 1 })
            .limit(ACCOUNTING_LIMIT)
            .max_time(quota::IO)
            .await
            .map_err(quota::unavailable)?;
        let mut page = Vec::new();
        while let Some(row) = rows.try_next().await.map_err(quota::unavailable)? {
            let source = quota::source_from_charge(&row)?;
            let key = row
                .get_str("_id")
                .map_err(|_| DocumentError::Corrupt)?
                .to_owned();
            let marker = self
                .ledger
                .find_one(doc! { "_id": object_authority::key(&source) })
                .max_time(quota::IO)
                .await
                .map_err(quota::unavailable)?;
            let mode = match marker {
                Some(row) if row.contains_key("authority_version") => {
                    let authority = Authority::decode(&row)?;
                    if authority.source != source {
                        return Err(DocumentError::Corrupt);
                    }
                    let due = match authority.state {
                        State::Reserved | State::Published => {
                            authority.updated_at <= mature_before(now)
                        }
                        State::DeletePending => authority
                            .lease
                            .as_ref()
                            .is_some_and(|lease| lease.until <= now),
                        State::Settled => false,
                    };
                    due.then_some(CandidateMode::Current(authority))
                }
                other if legacy_mature => Some(CandidateMode::Legacy(other)),
                _ => None,
            };
            page.push(AccountingEntry {
                key,
                candidate: mode.map(|mode| Candidate { source, mode }),
            });
        }
        Ok(page)
    }

    pub(super) async fn ready(&self) -> Result<bool, DocumentError> {
        let Some(state) = self
            .ledger
            .find_one(doc! { "_id": quota::STATE })
            .max_time(quota::IO)
            .await
            .map_err(quota::unavailable)?
        else {
            return Ok(false);
        };
        Ok(Binding::read(&state, &self.namespace).is_ok() && quota::generation_enabled(&state)?)
    }

    pub(super) async fn enable_generations(&self) -> Result<(), DocumentError> {
        let state = self
            .ledger
            .find_one(doc! { "_id": quota::STATE })
            .max_time(quota::IO)
            .await
            .map_err(quota::unavailable)?
            .filter(quota::ready)
            .ok_or(DocumentError::Unavailable)?;
        Binding::read(&state, &self.namespace)?;
        if !quota::generation_enabled(&state)? {
            let updated = self
                .ledger
                .update_one(
                    doc! { "_id": quota::STATE, "phase": "ready" },
                    doc! { "$set": { "source_generations": true } },
                )
                .await
                .map_err(quota::unavailable)?;
            if updated.matched_count != 1 {
                return Err(DocumentError::Unavailable);
            }
        }
        Ok(())
    }

    pub(super) async fn claim(
        &self,
        candidate: &Candidate,
        now: DateTime,
    ) -> Result<Option<DeleteClaim>, DocumentError> {
        self.transact(Action::Claim(candidate, now))
            .await
            .map(|outcome| match outcome {
                Outcome::Claim(claim) => claim,
                Outcome::Settled => unreachable!("claim has a claim outcome"),
            })
    }

    pub(super) async fn settle(
        &self,
        claim: &DeleteClaim,
        confirmation: ConfirmedDeletion,
    ) -> Result<(), DocumentError> {
        if !confirmation.matches(
            &claim.source.object_key(),
            claim.generation,
            &self.namespace,
        ) {
            return Err(DocumentError::Conflict);
        }
        self.transact(Action::Settle(claim)).await.map(|_| ())
    }

    async fn transact(&self, action: Action<'_>) -> Result<Outcome, DocumentError> {
        tokio::time::timeout(quota::IO, self.attempt(&action))
            .await
            .map_err(|_| DocumentError::Unavailable)?
    }

    async fn attempt(&self, action: &Action<'_>) -> Result<Outcome, DocumentError> {
        for _ in 0..5 {
            let mut session = self
                .ledger
                .client()
                .start_session()
                .await
                .map_err(quota::unavailable)?;
            match start_transaction(&mut session).await {
                Ok(()) => {}
                Err(error) if is_confirmed_body_retry(&error) => continue,
                Err(_) => return Err(DocumentError::Unavailable),
            }
            let outcome = match action {
                Action::Claim(candidate, now) => self
                    .claim_transaction(candidate, *now, &mut session)
                    .await
                    .map(Outcome::Claim),
                Action::Settle(claim) => self
                    .settle_transaction(claim, &mut session)
                    .await
                    .map(|()| Outcome::Settled),
            };
            let outcome = match outcome {
                Ok(outcome) => outcome,
                Err(error) => {
                    if session.abort_guarded().await.is_err() {
                        tracing::warn!(
                            "document reclamation transaction abort was not acknowledged"
                        );
                    }
                    match error {
                        quota::QuotaError::Driver(error) if is_confirmed_body_retry(&error) => {
                            continue
                        }
                        quota::QuotaError::Driver(_) => return Err(DocumentError::Unavailable),
                        quota::QuotaError::Domain(error) => return Err(error),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(outcome),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(DocumentError::Unavailable)
                }
            }
        }
        Err(DocumentError::Unavailable)
    }

    async fn claim_transaction(
        &self,
        candidate: &Candidate,
        now: DateTime,
        session: &mut ClientSession,
    ) -> Result<Option<DeleteClaim>, quota::QuotaError> {
        let binding = self.binding(session).await?;
        let source = &candidate.source;
        let key = object_authority::key(source);
        let row = self
            .ledger
            .find_one(doc! { "_id": &key })
            .session(&mut *session)
            .await?;
        let mut current = match &candidate.mode {
            CandidateMode::Current(expected) => {
                let Some(row) = &row else {
                    return Ok(None);
                };
                let current = Authority::decode(row)?;
                if &current != expected {
                    return Ok(None);
                }
                current
            }
            CandidateMode::Legacy(expected) => {
                if &row != expected {
                    return Ok(None);
                }
                if row
                    .as_ref()
                    .is_some_and(|row| row != &source_marker(source))
                {
                    return Err(DocumentError::Corrupt.into());
                }
                Authority::initial(source, row.is_some(), mature_before(now))
            }
        };
        let active = self.document(source, session).await?;
        if active.is_none()
            && current.state == State::Published
            && !reclamation_slots::terminal(&self.ledger, source, session).await?
        {
            // A published source without live metadata requires terminal deletion evidence.
            return Ok(None);
        }
        let referenced = active.as_ref().is_some_and(|record| {
            let key = source.object_key();
            record
                .revisions
                .iter()
                .any(|revision| revision.object_key == key)
        });
        if referenced
            || (current.state != State::DeletePending && current.updated_at > mature_before(now))
        {
            return Ok(None);
        }
        let charge = quota::object_row(source)?;
        if self
            .ledger
            .find_one(doc! { "_id": &charge["_id"] })
            .session(&mut *session)
            .await?
            .as_ref()
            != Some(&charge)
        {
            return Err(DocumentError::Corrupt.into());
        }
        // Generation-less and fenced publishers serialize here. The authority
        // CAS also protects a grant and its exact lease.
        if self.ledger.update_one(doc! {"_id":quota::owner_id(&source.owner_hash),"kind":"owner","document_schema_version":1_i64}, doc! {"$inc":{"fence":1_i64}}).session(&mut *session).await?.matched_count != 1 {
            return Err(DocumentError::Corrupt.into());
        }
        let holder = object_authority::reservation_id();
        let lease = Lease {
            holder: holder.clone(),
            until: DateTime::from_millis(now.timestamp_millis().saturating_add(LEASE_MS)),
        };
        let expected = match candidate.mode {
            CandidateMode::Current(_) => Some(current.condition()),
            CandidateMode::Legacy(Some(_)) => {
                Some(doc! { "_id": key, "document_schema_version": 1_i64,
                "authority_version": { "$exists": false } })
            }
            CandidateMode::Legacy(None) => None,
        };
        let generation = current.claim_delete(lease, now)?;
        if let Some(expected) = expected {
            self.replace(expected, &current, session).await?;
        } else {
            self.ledger
                .insert_one(current.encoded())
                .session(session)
                .await?;
        }
        Ok(Some(DeleteClaim {
            source: current.source,
            generation,
            holder,
            binding,
        }))
    }

    async fn replace(
        &self,
        expected: Document,
        authority: &Authority,
        session: &mut ClientSession,
    ) -> Result<(), quota::QuotaError> {
        if self
            .ledger
            .replace_one(expected, authority.encoded())
            .session(&mut *session)
            .await?
            .matched_count
            != 1
        {
            return Err(DocumentError::Conflict.into());
        }
        Ok(())
    }
}

enum Action<'a> {
    Claim(&'a Candidate, DateTime),
    Settle(&'a DeleteClaim),
}

enum Outcome {
    Claim(Option<DeleteClaim>),
    Settled,
}

fn mature_before(now: DateTime) -> DateTime {
    DateTime::from_millis(now.timestamp_millis().saturating_sub(AGE_MS))
}
