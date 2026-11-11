//! Optional title retry metadata; old Agent documents remain readable.
use layer2_core::sandbox::{AgentTitleClaim, AgentTitleStoreError, AGENT_TITLE_MAX_ATTEMPTS};
use mongodb::bson::{doc, Bson, DateTime, Document};

#[derive(Debug, Default)]
pub(super) struct TitleRetry {
    pub attempts: u32,
    pub next_at: Option<DateTime>,
    pub settled: bool,
}
impl TitleRetry {
    pub fn eligible(&self, now: DateTime) -> bool {
        !self.settled && self.next_at.is_none_or(|at| at <= now)
    }
}
pub(super) fn read(document: &Document) -> Result<TitleRetry, AgentTitleStoreError> {
    let corrupt = || AgentTitleStoreError::Corrupt;
    let attempts = match document.get("title_attempt_count") {
        None | Some(Bson::Null) => 0,
        Some(Bson::Int32(value)) => u32::try_from(*value).map_err(|_| corrupt())?,
        Some(Bson::Int64(value)) => u32::try_from(*value).map_err(|_| corrupt())?,
        _ => return Err(corrupt()),
    };
    if attempts > AGENT_TITLE_MAX_ATTEMPTS {
        return Err(corrupt());
    }
    let next_at = match document.get("title_next_attempt_at") {
        None | Some(Bson::Null) => None,
        Some(Bson::DateTime(at)) => Some(*at),
        _ => return Err(corrupt()),
    };
    let settled = match document.get("title_settled") {
        None | Some(Bson::Null) => false,
        Some(Bson::Boolean(value)) => *value,
        _ => return Err(corrupt()),
    };
    Ok(TitleRetry {
        attempts,
        next_at,
        settled,
    })
}
pub(super) fn claim_fields(claim: &AgentTitleClaim) -> Document {
    doc! { "title_attempt_count": i64::from(claim.attempts()),
    "title_next_attempt_at": DateTime::from_system_time(claim.next_attempt_at()), "title_settled": false }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_documents_and_unknown_future_fields_preserve_compatibility() {
        let old = doc! { "title_attempted": true, "title_source": "fallback", "future_field": 7 };
        let retry = read(&old).unwrap();
        assert_eq!(retry.attempts, 0);
        assert!(retry.eligible(DateTime::now()));
        let mut new = old.clone();
        new.extend(doc! { "title_attempt_count": 2_i64, "title_next_attempt_at": DateTime::from_millis(10000), "title_settled": false });
        let retry = read(&new).unwrap();
        assert!(!retry.eligible(DateTime::from_millis(9999)));
        assert!(retry.eligible(DateTime::from_millis(10000)));
        new.insert("title_settled", true);
        assert!(!read(&new).unwrap().eligible(DateTime::from_millis(20000)));
        assert_eq!(
            new.get_str("title_source").unwrap(),
            old.get_str("title_source").unwrap()
        );
        for bad in [-1_i64, 4, i64::MAX] {
            new.insert("title_attempt_count", bad);
            assert!(read(&new).is_err());
        }
    }
}
