use super::*;

impl MongoAgentOwnership {
    pub(super) async fn register_member(
        &self,
        member: Member,
    ) -> Result<Membership, OwnershipError> {
        records::validate_member(&member)?;
        if member.protocol != PROTOCOL_VERSION {
            return Err(OwnershipError::IncompatibleProtocol);
        }
        // Registration is single-use. An ambiguous response requires a fresh
        // process identity; a later restart cannot revive the old incarnation.
        let expires_at = self
            .server_now()
            .await?
            .checked_add(DEFAULT_LEASE_MS as i64)
            .ok_or_else(|| invalid("membership time overflow"))?;
        self.members
            .insert_one(doc! {
                "_id": &member.instance_id, "document_schema_version": 1_i64,
                "protocol": i64::from(member.protocol),
                "relay_address": &member.relay_address,
                "draining": false, "owned_agents": 0_i64,
                "max_owned_agents": i64::from(member.max_owned_agents),
                "expires_at": mongodb::bson::DateTime::from_millis(expires_at),
            })
            .await
            .map_err(unavailable)?;
        self.heartbeat_member(&member.instance_id, false).await
    }

    pub(super) async fn heartbeat_member(
        &self,
        instance_id: &str,
        draining: bool,
    ) -> Result<Membership, OwnershipError> {
        let mut filter = doc! {
            "_id": instance_id, "protocol": i64::from(PROTOCOL_VERSION),
            "$expr": {"$gt": ["$expires_at", "$$NOW"]},
        };
        if !draining {
            filter.insert("draining", false);
        }
        let updated = self
            .members
            .update_one(
                filter,
                vec![doc! {"$set": {
                    "expires_at": {"$add": ["$$NOW", DEFAULT_LEASE_MS as i64]},
                    "draining": draining,
                }}],
            )
            .await
            .map_err(unavailable)?;
        if updated.matched_count != 1 {
            return Err(OwnershipError::Stale);
        }
        self.membership().await
    }
}
