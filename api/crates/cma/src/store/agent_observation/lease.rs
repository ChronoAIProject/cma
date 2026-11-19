use super::*;

pub(super) struct Lease {
    pub token: AgentObservationToken,
    pub expires: i64,
}

pub(super) fn read(row: &Document) -> Result<Lease, AgentReadError> {
    let integer = |field| {
        row.get_i64(field)
            .or_else(|_| row.get_i32(field).map(i64::from))
            .map_err(unavailable)
    };
    if integer("document_schema_version")? != 1 {
        return Err(unavailable("unsupported read custody version"));
    }
    let epoch = u64::try_from(integer("epoch")?).map_err(unavailable)?;
    let id = row.get_str("_id").map_err(unavailable)?;
    let created = row.get_str("created_at").map_err(unavailable)?;
    let instance = row.get_str("instance_id").map_err(unavailable)?;
    let expires = row
        .get_datetime("expires_at")
        .map_err(unavailable)?
        .timestamp_millis();
    let next = row
        .get_datetime("next_refresh_at")
        .map_err(unavailable)?
        .timestamp_millis();
    row.get_str("round").map_err(unavailable)?;
    if (epoch == 0 && (expires != 0 || next != 0))
        || id.is_empty()
        || id.len() > 256
        || instance.is_empty()
        || instance.len() > 256
        || layer2_core::seconds_from_rfc3339(created).is_none()
        || next > expires
    {
        return Err(unavailable("invalid read custody"));
    }
    Ok(Lease {
        token: AgentObservationToken {
            agent_id: AgentId(id.into()),
            created_at: created.into(),
            instance_id: instance.into(),
            epoch,
        },
        expires,
    })
}

pub(super) fn filter(token: &AgentObservationToken) -> Result<Document, AgentReadError> {
    Ok(
        doc! {"_id": token.agent_id.as_str(), "created_at": &token.created_at, "instance_id": &token.instance_id, "epoch": i64::try_from(token.epoch).map_err(unavailable)?, "document_schema_version": 1_i64},
    )
}

pub(super) fn destination(
    lease: Lease,
    created_at: &str,
    membership: &layer2_core::agent_ownership::Membership,
    started: Instant,
) -> Result<ObservationDestination, AgentReadError> {
    if lease.token.created_at != created_at {
        return Err(unavailable("read custody incarnation changed"));
    }
    let member = membership
        .members
        .iter()
        .find(|member| {
            member.instance_id == lease.token.instance_id
                && member.is_live(membership.now_ms)
                && !member.draining
        })
        .ok_or_else(|| unavailable("read owner unavailable"))?;
    let remaining = lease
        .expires
        .min(member.expires_at_ms)
        .saturating_sub(membership.now_ms);
    if remaining <= 0 {
        return Err(unavailable("read custody expired"));
    }
    let authority = AgentObservationAuthority::new(
        lease.token,
        started + Duration::from_millis(remaining as u64),
    );
    if !authority.current() {
        return Err(unavailable("read custody expired during routing"));
    }
    Ok(ObservationDestination {
        member: member.clone(),
        authority,
    })
}
