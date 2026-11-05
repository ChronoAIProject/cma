use super::*;

pub(super) async fn current(
    database: &Database,
    agent_id: &str,
    session: &mut ClientSession,
) -> Result<(layer2_core::sandbox::Agent, Head), SandboxStoreError> {
    let aggregate = database
        .collection::<Document>("agents")
        .find_one(doc! {"_id":agent_id})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    let agent = read_agent(&aggregate)?;
    let (incarnation, revision) = read_format(&aggregate)?.ok_or_else(corrupt)?;
    let current = Head::read(
        database
            .collection::<Document>(HEADS)
            .find_one(doc! {"_id":agent_id})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?,
    )?;
    if current.agent_id != agent_id
        || current.incarnation != incarnation
        || current.revision != revision
        || u64::try_from(current.last_seq).ok() != Some(agent.conversation.last_seq)
    {
        return Err(corrupt());
    }
    Ok((agent, current))
}

pub(super) async fn claim(
    database: &Database,
    agent_id: &str,
    audit_id: &str,
    cancel: bool,
    session: &mut ClientSession,
) -> Result<Progress, SandboxStoreError> {
    let now = bson::DateTime::now().timestamp_millis();
    let (agent, current) = current(database, agent_id, session).await?;
    let incarnation = current.incarnation.as_str();
    let collection = database.collection::<Document>(AUDITS);
    // Length prefixes prevent delimiter aliases between independent identities.
    let id = format!("{}:{agent_id}{}:{audit_id}", agent_id.len(), audit_id.len());
    let existing = collection
        .find_one(doc! {"_id":&id})
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut progress: Progress = match &existing {
        Some(document) => {
            bson::deserialize_from_document(document.clone()).map_err(|_| corrupt())?
        }
        None => Progress {
            id: id.clone(),
            format_version: 1,
            agent_id: agent_id.into(),
            incarnation: incarnation.into(),
            audit_id: audit_id.into(),
            holder: String::new(),
            expires_at_ms: 0,
            fence: 0,
            status: "running".into(),
            through_seq: current.last_seq,
            head_revision: current.revision,
            next_ordinal: 0,
            checked_entries: 0,
            checked_bytes: 0,
            previous_seal: None,
            captured_head: current.clone(),
            captured_agent: agent.clone(),
            semantic: Default::default(),
        },
    };
    progress.captured_head.validate()?;
    if progress.format_version != 1
        || progress.id != id
        || progress.agent_id != agent_id
        || progress.incarnation != incarnation
        || progress.audit_id != audit_id
        || progress.through_seq != progress.captured_head.last_seq
        || progress.head_revision != progress.captured_head.revision
        || progress.captured_head.incarnation != progress.incarnation
        || progress.captured_head.agent_id != progress.agent_id
        || progress.captured_agent.id.as_str() != progress.agent_id
        || digest::incarnation(&progress.captured_agent) != progress.incarnation
        || i64::try_from(progress.captured_agent.conversation.last_seq).ok()
            != Some(progress.through_seq)
        || i64::try_from(progress.semantic.through_seq).ok() != Some(progress.checked_entries)
        || progress.next_ordinal < 0
        || progress.next_ordinal > progress.captured_head.active_ordinal + 1
        || progress.checked_entries < 0
        || progress.checked_bytes < 0
        || progress.fence < 0
    {
        return Err(corrupt());
    }
    if matches!(
        progress.status.as_str(),
        "complete" | "installed" | "cancelled" | "corrupt"
    ) {
        return Ok(progress);
    }
    if progress.status != "running" {
        return Err(corrupt());
    }
    if progress.expires_at_ms > now {
        return Err(SandboxStoreError::Conflict);
    }
    let expected_fence = progress.fence;
    progress.fence = expected_fence.checked_add(1).ok_or_else(corrupt)?;
    progress.holder = uuid::Uuid::new_v4().to_string();
    progress.expires_at_ms = now.checked_add(30_000).ok_or_else(corrupt)?;
    if cancel {
        progress.status = "cancelled".into();
        progress.expires_at_ms = 0;
    }
    let document = bson::serialize_to_document(&progress).map_err(|_| corrupt())?;
    if existing.is_some() {
        let result = collection
            .replace_one(
                doc! {"_id":&id,"fence":expected_fence,"expires_at_ms":{"$lte":now}},
                document,
            )
            .session(session)
            .await
            .map_err(unavailable)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
    } else {
        collection
            .insert_one(document)
            .session(session)
            .await
            .map_err(unavailable)?;
    }
    Ok(progress)
}
