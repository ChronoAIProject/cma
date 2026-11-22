//! Owner decoder and value types frozen from develop@e96070898 for rollback proof.
use super::*;

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct PredecessorToken {
    agent_id: AgentId,
    instance_id: String,
    epoch: u64,
    protocol: u32,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct PredecessorLease {
    token: PredecessorToken,
    expires_at_ms: i64,
}

impl PredecessorLease {
    pub(super) fn decode(row: &Document) -> Result<Self, OwnershipError> {
        // The predecessor reads BSON Document, then projects known fields; it
        // does not deserialize the whole owner row through a strict struct.
        if row.get_i64("document_schema_version").map_err(invalid)? != 1 {
            return Err(invalid("unsupported document schema version"));
        }
        let token = PredecessorToken {
            agent_id: AgentId(row.get_str("_id").map_err(invalid)?.into()),
            instance_id: row.get_str("instance_id").map_err(invalid)?.into(),
            epoch: u64::try_from(row.get_i64("epoch").map_err(invalid)?).map_err(invalid)?,
            protocol: u32::try_from(row.get_i64("protocol").map_err(invalid)?).map_err(invalid)?,
        };
        if token.epoch == 0 || token.instance_id.is_empty() {
            return Err(invalid("invalid ownership token"));
        }
        if token.protocol != 1 {
            return Err(OwnershipError::IncompatibleProtocol);
        }
        Ok(Self {
            token,
            expires_at_ms: row
                .get_datetime("expires_at")
                .map_err(invalid)?
                .timestamp_millis(),
        })
    }
}
