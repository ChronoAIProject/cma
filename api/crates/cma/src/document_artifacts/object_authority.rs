//! Persistent authority for a single content-addressed source key.
use layer2_core::document_artifacts::{
    valid_digest, DocumentError, DocumentSourceReservation, MAX_SOURCE_GENERATION,
};
use mongodb::bson::{doc, DateTime, Document};

pub(super) const AUTHORITY_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    Reserved,
    Published,
    DeletePending,
    Settled,
}

impl State {
    fn parse(value: &str) -> Result<Self, DocumentError> {
        match value {
            "reserved" => Ok(Self::Reserved),
            "published" => Ok(Self::Published),
            "delete_pending" => Ok(Self::DeletePending),
            "settled" => Ok(Self::Settled),
            _ => Err(DocumentError::Corrupt),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Published => "published",
            Self::DeletePending => "delete_pending",
            Self::Settled => "settled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Authority {
    pub source: DocumentSourceReservation,
    pub state: State,
    pub generation: u64,
    pub reservation: String,
    pub fence: u64,
    pub delete_generation: Option<u64>,
    pub updated_at: DateTime,
    pub lease: Option<Lease>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Lease {
    pub holder: String,
    pub until: DateTime,
}

impl Authority {
    pub(super) fn initial(
        source: &DocumentSourceReservation,
        published: bool,
        now: DateTime,
    ) -> Self {
        Self {
            source: source.clone(),
            state: if published {
                State::Published
            } else {
                State::Reserved
            },
            generation: 1,
            reservation: reservation_id(),
            fence: 1,
            delete_generation: None,
            updated_at: now,
            lease: None,
        }
    }

    pub(super) fn decode(row: &Document) -> Result<Self, DocumentError> {
        let source = DocumentSourceReservation {
            owner_hash: string(row, "owner_hash")?.to_owned(),
            document_id: string(row, "document_id")?.to_owned(),
            sha256: string(row, "sha256")?.to_owned(),
            bytes: super::quota::number(row, "bytes")?,
        };
        source.validate().map_err(|_| DocumentError::Corrupt)?;
        if string(row, "_id")? != key(&source) || string(row, "kind")? != "source_commit" {
            return Err(DocumentError::Corrupt);
        }
        let state = State::parse(string(row, "state")?)?;
        let generation = super::quota::number(row, "generation")?;
        let fence = super::quota::number(row, "fence")?;
        let reservation = string(row, "reservation")?.to_owned();
        let delete_generation = row
            .contains_key("delete_generation")
            .then(|| super::quota::number(row, "delete_generation"))
            .transpose()?;
        let updated_at = *row
            .get_datetime("updated_at")
            .map_err(|_| DocumentError::Corrupt)?;
        let lease = if row.contains_key("claim_holder") || row.contains_key("claim_until") {
            let holder = string(row, "claim_holder")?.to_owned();
            let until = *row
                .get_datetime("claim_until")
                .map_err(|_| DocumentError::Corrupt)?;
            if !valid_digest(&holder) {
                return Err(DocumentError::Corrupt);
            }
            Some(Lease { holder, until })
        } else {
            None
        };
        if row.get_i64("document_schema_version").ok() != Some(1)
            || row.get_i64("authority_version").ok() != Some(AUTHORITY_VERSION)
            || generation == 0
            || generation > MAX_SOURCE_GENERATION
            || delete_generation.is_some_and(|value| value > MAX_SOURCE_GENERATION)
            || fence == 0
            || !valid_digest(&reservation)
            || (state == State::DeletePending) != lease.is_some()
            || match state {
                State::DeletePending | State::Settled => {
                    delete_generation != generation.checked_add(1)
                }
                State::Reserved | State::Published => delete_generation.is_some(),
            }
        {
            return Err(DocumentError::Corrupt);
        }
        Ok(Self {
            source,
            state,
            generation,
            reservation,
            fence,
            delete_generation,
            updated_at,
            lease,
        })
    }

    pub(super) fn condition(&self) -> Document {
        doc! {
            "_id": key(&self.source),
            "document_schema_version": 1_i64, "authority_version": AUTHORITY_VERSION,
            "state": self.state.as_str(),
            "generation": self.generation as i64,
            "reservation": &self.reservation,
            "fence": self.fence as i64,
        }
    }

    pub(super) fn encoded(&self) -> Document {
        let mut row = doc! {
            "_id": key(&self.source),
            "document_schema_version": 1_i64, "authority_version": AUTHORITY_VERSION,
            "kind": "source_commit",
            "owner_hash": &self.source.owner_hash, "document_id": &self.source.document_id,
            "sha256": &self.source.sha256, "bytes": self.source.bytes as i64,
            "state": self.state.as_str(), "generation": self.generation as i64,
            "reservation": &self.reservation, "fence": self.fence as i64,
            "updated_at": self.updated_at,
        };
        if let Some(generation) = self.delete_generation {
            row.insert("delete_generation", generation as i64);
        }
        if let Some(lease) = &self.lease {
            row.insert("claim_holder", &lease.holder);
            row.insert("claim_until", lease.until);
        }
        row
    }

    pub(super) fn renew(&mut self, now: DateTime) -> Result<(), DocumentError> {
        if !matches!(self.state, State::Reserved | State::Published) {
            return Err(DocumentError::Unavailable);
        }
        self.fence = advance(self.fence)?;
        self.updated_at = self.updated_at.max(now);
        Ok(())
    }

    pub(super) fn publish(
        &mut self,
        generation: u64,
        reservation: &str,
        now: DateTime,
    ) -> Result<(), DocumentError> {
        if self.generation != generation
            || self.reservation != reservation
            || !matches!(self.state, State::Reserved | State::Published)
        {
            return Err(DocumentError::Conflict);
        }
        self.renew(now)?;
        self.state = State::Published;
        Ok(())
    }

    pub(super) fn claim_delete(
        &mut self,
        lease: Lease,
        now: DateTime,
    ) -> Result<u64, DocumentError> {
        if !valid_digest(&lease.holder) || lease.until <= now {
            return Err(DocumentError::Invalid);
        }
        let generation = match self.state {
            State::Reserved | State::Published => advance(self.generation)?,
            State::DeletePending
                if self
                    .lease
                    .as_ref()
                    .is_some_and(|active| active.until <= now) =>
            {
                self.delete_generation.ok_or(DocumentError::Corrupt)?
            }
            _ => return Err(DocumentError::Conflict),
        };
        self.fence = advance(self.fence)?;
        self.delete_generation = Some(generation);
        self.state = State::DeletePending;
        self.lease = Some(lease);
        Ok(generation)
    }

    pub(super) fn settle(&mut self, generation: u64, holder: &str) -> Result<(), DocumentError> {
        if self.state != State::DeletePending
            || self.delete_generation != Some(generation)
            || self
                .lease
                .as_ref()
                .is_none_or(|lease| lease.holder != holder)
        {
            return Err(DocumentError::Conflict);
        }
        self.fence = advance(self.fence)?;
        self.state = State::Settled;
        self.lease = None;
        Ok(())
    }

    pub(super) fn recreate(&mut self, now: DateTime) -> Result<(), DocumentError> {
        if self.state != State::Settled {
            return Err(DocumentError::Conflict);
        }
        self.generation = advance(self.delete_generation.ok_or(DocumentError::Corrupt)?)?;
        self.fence = advance(self.fence)?;
        self.state = State::Reserved;
        self.delete_generation = None;
        self.reservation = reservation_id();
        self.updated_at = now;
        Ok(())
    }
}

pub(super) fn reservation_id() -> String {
    layer2_core::document_artifacts::digest(uuid::Uuid::new_v4().as_bytes())
}

fn advance(value: u64) -> Result<u64, DocumentError> {
    value
        .checked_add(1)
        .filter(|next| *next <= MAX_SOURCE_GENERATION)
        .ok_or(DocumentError::Capacity)
}

fn string<'a>(row: &'a Document, field: &str) -> Result<&'a str, DocumentError> {
    row.get_str(field).map_err(|_| DocumentError::Corrupt)
}

pub(super) fn key(source: &DocumentSourceReservation) -> String {
    format!(
        "source_commit:{}:{}:{}",
        source.owner_hash, source.document_id, source.sha256
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::document_artifacts::digest;

    #[test]
    fn a_lost_delete_keeps_its_generation_through_takeover_and_recreation() {
        let source = DocumentSourceReservation {
            owner_hash: digest(b"owner"),
            document_id: "doc_00000000000000000000000000000001".into(),
            sha256: digest(b"body"),
            bytes: 4,
        };
        let at = DateTime::from_millis;
        let mut authority = Authority::initial(&source, true, at(1));
        let original_upload = authority.reservation.clone();
        let first = Lease {
            holder: digest(b"first"),
            until: at(5),
        };
        let second = Lease {
            holder: digest(b"second"),
            until: at(9),
        };
        let deleting = authority.claim_delete(first.clone(), at(2)).unwrap();
        let mut authority = Authority::decode(&authority.encoded()).unwrap();
        assert_eq!(authority.renew(at(3)), Err(DocumentError::Unavailable));
        assert_eq!(
            authority.publish(1, &original_upload, at(3)),
            Err(DocumentError::Conflict)
        );
        assert_eq!(
            authority.claim_delete(second.clone(), at(4)),
            Err(DocumentError::Conflict)
        );
        assert_eq!(authority.claim_delete(second.clone(), at(5)), Ok(deleting));
        assert_eq!(
            authority.settle(deleting, &first.holder),
            Err(DocumentError::Conflict)
        );
        authority.settle(deleting, &second.holder).unwrap();

        let mut authority = Authority::decode(&authority.encoded()).unwrap();
        authority.recreate(at(6)).unwrap();
        assert!(authority.generation > deleting);
        assert_ne!(authority.reservation, original_upload);
        let before = authority.clone();
        assert_eq!(
            authority.settle(deleting, &second.holder),
            Err(DocumentError::Conflict)
        );
        assert_eq!(authority, before);
        let reservation = authority.reservation.clone();
        authority
            .publish(authority.generation, &reservation, at(7))
            .unwrap();
        assert_eq!(authority.state, State::Published);
    }
}
