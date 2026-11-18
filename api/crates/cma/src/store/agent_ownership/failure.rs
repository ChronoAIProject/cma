//! Additive diagnostics outside the predecessor reader's authority projection.
use super::*;
use layer2_core::agent_ownership::bounded;
use mongodb::bson::Bson;
use sha2::{Digest, Sha256};

impl MongoAgentOwnership {
    pub(super) async fn record_failure(
        &self,
        token: &OwnershipToken,
        failure: OwnershipFailure,
    ) -> Result<(), OwnershipError> {
        if !self.receiver_diagnostics_enabled {
            return Err(OwnershipError::Unavailable(
                "receiver diagnostics are disabled".into(),
            ));
        }
        let filter = token_filter(token)?;
        let update = failure_update(token, failure)?;
        bounded(async {
            let result = self
                .owners
                .update_one(filter, update)
                .await
                .map_err(unavailable)?;
            if result.matched_count == 1 {
                Ok(())
            } else {
                Err(OwnershipError::Stale)
            }
        })
        .await
    }
}

fn failure_update(
    token: &OwnershipToken,
    failure: OwnershipFailure,
) -> Result<Vec<Document>, OwnershipError> {
    let epoch = i64::try_from(token.epoch).map_err(invalid)?;
    let instance_sha256 = format!("{:x}", Sha256::digest(token.instance_id.as_bytes()));
    // BSON integers cannot represent the entire advertised u64 lease range.
    // Decimal128 retains it exactly, including deliberately incompatible values.
    let lease_ms = failure
        .lease_ms
        .map(|lease| {
            lease
                .to_string()
                .parse()
                .map(Bson::Decimal128)
                .map_err(invalid)
        })
        .transpose()?
        .unwrap_or(Bson::Null);
    let record = doc! {
        "owner_epoch": epoch,
        "owner_instance_sha256": &instance_sha256,
        "owner_protocol": i64::from(token.protocol),
        "phase": failure.phase.as_str(), "reason": failure.reason.as_str(),
        "http_status": failure.http_status.map(i32::from),
        "protocol": failure.protocol.map(i64::from),
        "lease_ms": lease_ms,
    };
    Ok(vec![doc! {"$set": {"receiver_failure": {"$cond": [
        {"$and": [
            {"$eq": ["$receiver_failure.owner_epoch", epoch]},
            {"$eq": ["$receiver_failure.owner_instance_sha256", {"$literal": instance_sha256}]},
            {"$eq": ["$receiver_failure.owner_protocol", i64::from(token.protocol)]},
        ]},
        "$receiver_failure",
        {"$mergeObjects": [{"$literal": record}, {"observed_at": "$$NOW"}]},
    ]}}}])
}

pub(super) fn diagnostics_enabled(
    value: Result<String, std::env::VarError>,
) -> Result<bool, OwnershipError> {
    match value.as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("false") => Ok(false),
        Ok("true") => Ok(true),
        _ => Err(invalid(
            "CHRONO_AGENT_MANAGER_OWNERSHIP_DIAGNOSTICS_ENABLED must be true or false",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_diagnostics_require_explicit_true_without_relaxing_invalid_configuration() {
        assert_eq!(
            diagnostics_enabled(Err(std::env::VarError::NotPresent)),
            Ok(false)
        );
        assert_eq!(diagnostics_enabled(Ok("false".into())), Ok(false));
        assert_eq!(diagnostics_enabled(Ok("true".into())), Ok(true));
        for invalid in ["", "1", "TRUE", " true", "false "] {
            assert!(diagnostics_enabled(Ok(invalid.into())).is_err());
        }
        assert!(
            diagnostics_enabled(Err(std::env::VarError::NotUnicode("invalid".into()))).is_err()
        );
    }
}
