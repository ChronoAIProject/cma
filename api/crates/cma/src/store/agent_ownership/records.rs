use super::*;

pub(super) fn member_from(row: &Document) -> Result<Member, OwnershipError> {
    super::super::sandbox_mongo::require_version(row).map_err(invalid)?;
    let member = Member {
        instance_id: row.get_str("_id").map_err(invalid)?.into(),
        protocol: u32::try_from(row.get_i64("protocol").map_err(invalid)?).map_err(invalid)?,
        relay_address: row.get_str("relay_address").map_err(invalid)?.into(),
        expires_at_ms: row
            .get_datetime("expires_at")
            .map_err(invalid)?
            .timestamp_millis(),
        draining: row.get_bool("draining").map_err(invalid)?,
        owned_agents: u32::try_from(row.get_i64("owned_agents").map_err(invalid)?)
            .map_err(invalid)?,
        max_owned_agents: u32::try_from(row.get_i64("max_owned_agents").map_err(invalid)?)
            .map_err(invalid)?,
    };
    validate_member(&member)?;
    Ok(member)
}

pub(super) fn validate_member(member: &Member) -> Result<(), OwnershipError> {
    if member.instance_id.is_empty() || member.instance_id.len() > 128 {
        return Err(invalid("invalid instance identity"));
    }
    let address = url::Url::parse(&member.relay_address).map_err(|_| invalid("relay address"))?;
    if member.relay_address.len() > 2_048
        || !matches!(address.scheme(), "http" | "https")
        || address.host().is_none()
        || !address.username().is_empty()
        || address.password().is_some()
        || address.query().is_some()
        || address.fragment().is_some()
        || address.path() != "/"
        || member.max_owned_agents == 0
    {
        return Err(invalid("invalid membership configuration"));
    }
    Ok(())
}

pub(super) fn owner_from(row: &Document) -> Result<OwnershipLease, OwnershipError> {
    super::super::sandbox_mongo::require_version(row).map_err(invalid)?;
    let token = OwnershipToken {
        agent_id: AgentId(row.get_str("_id").map_err(invalid)?.into()),
        instance_id: row.get_str("instance_id").map_err(invalid)?.into(),
        epoch: u64::try_from(row.get_i64("epoch").map_err(invalid)?).map_err(invalid)?,
        protocol: u32::try_from(row.get_i64("protocol").map_err(invalid)?).map_err(invalid)?,
    };
    if token.epoch == 0 || token.instance_id.is_empty() {
        return Err(invalid("invalid ownership token"));
    }
    if token.protocol != PROTOCOL_VERSION {
        return Err(OwnershipError::IncompatibleProtocol);
    }
    Ok(OwnershipLease {
        token,
        expires_at_ms: row
            .get_datetime("expires_at")
            .map_err(invalid)?
            .timestamp_millis(),
    })
}

pub(super) fn token_filter(token: &OwnershipToken) -> Result<Document, OwnershipError> {
    if token.protocol != PROTOCOL_VERSION {
        return Err(OwnershipError::IncompatibleProtocol);
    }
    if token.epoch == 0 {
        return Err(OwnershipError::Stale);
    }
    Ok(doc! {
        "_id": token.agent_id.as_str(), "instance_id": &token.instance_id,
        "protocol": i64::from(token.protocol),
        "epoch": i64::try_from(token.epoch).map_err(invalid)?,
        "$expr": {"$gt": ["$expires_at", "$$NOW"]},
    })
}
