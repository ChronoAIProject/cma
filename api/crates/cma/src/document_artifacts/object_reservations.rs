//! Upgrade and renew per-key upload authority within the quota reservation transaction.
use super::{
    object_authority::{self, Authority, State},
    quota::QuotaError,
    source_commit::source_marker,
};
use layer2_core::document_artifacts::{DocumentError, DocumentSourceReservation};
use mongodb::{
    bson::{doc, DateTime, Document},
    ClientSession, Collection,
};

pub(super) async fn reserve(
    ledger: &Collection<Document>,
    source: &DocumentSourceReservation,
    charged: bool,
    generation_writes: bool,
    session: &mut ClientSession,
) -> Result<Option<Authority>, QuotaError> {
    let key = object_authority::key(source);
    let existing = ledger
        .find_one(doc! { "_id": &key })
        .session(&mut *session)
        .await?;
    let now = DateTime::now();
    let next = match existing {
        Some(row) if row.contains_key("authority_version") => {
            let mut authority = Authority::decode(&row)?;
            if authority.source != *source || charged == (authority.state == State::Settled) {
                return Err(DocumentError::Corrupt.into());
            }
            let expected = authority.condition();
            if authority.state == State::Settled {
                authority.recreate(now)?;
            } else {
                authority.renew(now)?;
            }
            let updated = ledger
                .replace_one(expected, authority.encoded())
                .session(&mut *session)
                .await?;
            if updated.matched_count != 1 {
                return Err(DocumentError::Unavailable.into());
            }
            authority
        }
        Some(row) => {
            if row != source_marker(source) || !charged {
                return Err(DocumentError::Corrupt.into());
            }
            if !generation_writes {
                return Ok(None);
            }
            let authority = Authority::initial(source, true, now);
            let upgraded = ledger
                .replace_one(
                    doc! { "_id": &key, "document_schema_version": 1_i64,
                    "authority_version": { "$exists": false } },
                    authority.encoded(),
                )
                .session(&mut *session)
                .await?;
            if upgraded.matched_count != 1 {
                return Err(DocumentError::Unavailable.into());
            }
            authority
        }
        None if generation_writes => {
            let authority = Authority::initial(source, false, now);
            ledger
                .insert_one(authority.encoded())
                .session(session)
                .await?;
            authority
        }
        None => return Ok(None),
    };
    Ok(Some(next))
}
